//! The firewall table of a sharing session, as typed values for the firewall's own netlink writer (`iyakki_fw::nl::tablegen`).
//! No `nft` program and no shell string is involved. The table is `inet iyakki_share`; sharing only ever creates and deletes that one.
//!
//! * `input` (priority -10): from the shared interface only DHCP (UDP 67) and DNS (UDP 53) are accepted; everything else is dropped, so a
//!   device on the cable cannot reach any service of this computer. (The Firewall's own zone, `iyakki_fw::service::set_share_zone`,
//!   says the same for its table, which has the base drop policy.)
//! * `forward` (priority -10, policy accept so other forwarding such as containers is untouched): replies come back from the uplink;
//!   devices cannot reach each other (isolation); devices cannot reach the private ranges behind the uplink unless allowed; devices
//!   reach the uplink with an address of the shared network; nothing else goes from or to the shared interface.
//! * `postrouting` (nat, priority 100): masquerade for the shared network going out of the uplink.

use super::plan::Plan;
use super::{BRIDGE_TABLE, TABLE};
use iyakki_fw::nl::tablegen::{ChainKind, ChainSpec, Expr, Family, Hook, IfName, TableSpec, CT_ESTABLISHED_RELATED};
use std::net::IpAddr;

/// The ranges that mean "a local network behind the uplink".
const PRIVATE: [([u8; 4], u8); 5] = [([10, 0, 0, 0], 8), ([172, 16, 0, 0], 12), ([192, 168, 0, 0], 16), ([169, 254, 0, 0], 16), ([100, 64, 0, 0], 10)];

pub fn table_spec(p: &Plan) -> TableSpec {
    let down = || IfName::Exact(p.down.clone());
    let up = || IfName::Exact(p.uplink.clone());
    let net = IpAddr::V4(std::net::Ipv4Addr::from(p.net.net.network()));
    let gw = IpAddr::V4(p.gateway());
    let input = vec![
        vec![Expr::IifIs(down()), Expr::L4(17), Expr::Dport(67), Expr::Accept],
        vec![Expr::IifIs(down()), Expr::L4(17), Expr::Dport(53), Expr::Daddr(gw, 32), Expr::Accept],
        vec![Expr::IifIs(down()), Expr::Drop],
    ];
    let mut forward = vec![vec![Expr::IifIs(up()), Expr::OifIs(down()), Expr::CtState(CT_ESTABLISHED_RELATED), Expr::Accept]];
    if p.isolate {
        forward.push(vec![Expr::IifIs(down()), Expr::OifIs(down()), Expr::Drop]);
    }
    if !p.allow_lan {
        for (a, bits) in PRIVATE {
            forward.push(vec![Expr::IifIs(down()), Expr::OifIs(up()), Expr::Daddr(IpAddr::V4(a.into()), bits), Expr::Drop]);
        }
    }
    forward.push(vec![Expr::IifIs(down()), Expr::OifIs(up()), Expr::Saddr(net, 24), Expr::Accept]);
    forward.push(vec![Expr::IifIs(down()), Expr::Drop]);
    forward.push(vec![Expr::OifIs(down()), Expr::Drop]);
    let post = vec![vec![Expr::OifIs(up()), Expr::Saddr(net, 24), Expr::Masquerade]];
    TableSpec {
        name: TABLE.into(),
        sets: vec![],
        chains: vec![
            ChainSpec { name: "input".into(), kind: ChainKind::Filter, hook: Hook::Input, priority: -10, policy_accept: true, rules: input },
            ChainSpec { name: "forward".into(), kind: ChainKind::Filter, hook: Hook::Forward, priority: -10, policy_accept: true, rules: forward },
            ChainSpec { name: "postrouting".into(), kind: ChainKind::Nat, hook: Hook::Postrouting, priority: 100, policy_accept: true, rules: post },
        ],
    }
}

/// The bridge-family table for a downstream that is a bridge of several ports (a Bluetooth access point: one port per phone), when
/// devices must not reach each other: every frame that comes in on a member port and is forwarded to another port of the bridge is
/// dropped, so the phones cannot see each other at layer 2 either (the `inet` table only sees what is routed). Traffic to this computer
/// and out to the uplink goes through the bridge's own address (input and routing), not the bridge forward hook, so it is untouched.
/// `None` when the downstream is a single port or isolation is off.
pub fn bridge_table_spec(p: &Plan) -> Option<TableSpec> {
    if !p.isolate || p.members.is_empty() {
        return None;
    }
    let rules = p.members.iter().map(|m| vec![Expr::IifIs(IfName::Exact(m.clone())), Expr::Drop]).collect();
    Some(TableSpec {
        name: BRIDGE_TABLE.into(),
        sets: vec![],
        chains: vec![ChainSpec { name: "forward".into(), kind: ChainKind::Filter, hook: Hook::Forward, priority: -200, policy_accept: true, rules }],
    })
}

fn name(n: &IfName) -> String {
    match n {
        IfName::Exact(s) => s.clone(),
        IfName::Prefix(s) => format!("{s}*"),
    }
}

fn word(e: &Expr) -> String {
    match e {
        Expr::IifIs(n) => format!("iif {}", name(n)),
        Expr::IifNot(n) => format!("iif != {}", name(n)),
        Expr::OifIs(n) => format!("oif {}", name(n)),
        Expr::OifNot(n) => format!("oif != {}", name(n)),
        Expr::CtState(_) => "ct state established,related".into(),
        Expr::Saddr(ip, p) => format!("saddr {ip}/{p}"),
        Expr::Daddr(ip, p) => format!("daddr {ip}/{p}"),
        Expr::DaddrSet(s) => format!("daddr @{s}"),
        Expr::DaddrDportSet(s) => format!("daddr . dport @{s}"),
        Expr::L4(6) => "tcp".into(),
        Expr::L4(17) => "udp".into(),
        Expr::L4(n) => format!("proto {n}"),
        Expr::Sport(p) => format!("sport {p}"),
        Expr::Dport(p) => format!("dport {p}"),
        Expr::MarkIs(m) => format!("mark {m:#x}"),
        Expr::SetMark(m) => format!("mark set {m:#x}"),
        Expr::SkuidIs(u) => format!("skuid {u}"),
        Expr::Accept => "accept".into(),
        Expr::Drop => "drop".into(),
        Expr::Return => "return".into(),
        Expr::RedirectPort(p) => format!("redirect {p}"),
        Expr::Masquerade => "masquerade".into(),
        Expr::DaddrRange(a, b) => format!("daddr {a}-{b}"),
        Expr::DportRange(a, b) => format!("dport {a}-{b}"),
        Expr::Nfproto(v4) => if *v4 { "ipv4" } else { "ipv6" }.into(),
        Expr::IcmpType(a, b) => format!("icmpv6 type {a}-{b}"),
        Expr::CtStatusDnat => "ct status dnat".into(),
        Expr::Counter => "counter".into(),
        Expr::Reject => "reject".into(),
        Expr::Dnat(ip, p) => format!("dnat to {ip}:{p}"),
    }
}

/// The table as text, one rule per line (what `nft list table` would show, for the golden test and for support).
pub fn describe(t: &TableSpec) -> String {
    describe_in(Family::Inet, t)
}

/// [`describe`] for a table of the given family.
pub fn describe_in(family: Family, t: &TableSpec) -> String {
    let mut s = format!("table {} {}\n", family.word(), t.name);
    for c in &t.chains {
        let hook = match c.hook {
            Hook::Input => "input",
            Hook::Forward => "forward",
            Hook::Output => "output",
            Hook::Postrouting => "postrouting",
            Hook::Prerouting => "prerouting",
        };
        s += &format!("  chain {} {} hook {hook} priority {} policy {}\n", c.name, if c.kind == ChainKind::Nat { "nat" } else { "filter" }, c.priority, if c.policy_accept { "accept" } else { "drop" });
        for r in &c.rules {
            s += &format!("    {}\n", r.iter().map(word).collect::<Vec<_>>().join(" "));
        }
    }
    s
}

/// The table of the older, single-device flow (`iyakki usb share-internet --apply`: this computer gives a cable to the device in
/// developer mode, fixed pair 10.77.0.1 / 10.77.0.2): forward the cable to the uplink, replies back, masquerade. Typed, written by the
/// same writer as the sessions of this module; it replaces the `nft` shell lines.
pub fn legacy_table(link: &str, uplink: &str) -> TableSpec {
    let (l, u) = (|| IfName::Exact(link.to_string()), || IfName::Exact(uplink.to_string()));
    TableSpec {
        name: TABLE.into(),
        sets: vec![],
        chains: vec![
            ChainSpec {
                name: "forward".into(),
                kind: ChainKind::Filter,
                hook: Hook::Forward,
                priority: -10,
                policy_accept: true,
                rules: vec![vec![Expr::IifIs(l()), Expr::OifIs(u()), Expr::Accept], vec![Expr::IifIs(u()), Expr::OifIs(l()), Expr::CtState(CT_ESTABLISHED_RELATED), Expr::Accept]],
            },
            ChainSpec { name: "post".into(), kind: ChainKind::Nat, hook: Hook::Postrouting, priority: 100, policy_accept: true, rules: vec![vec![Expr::IifIs(l()), Expr::OifIs(u()), Expr::Masquerade]] },
        ],
    }
}
