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
use super::TABLE;
use iyakki_fw::nl::tablegen::{ChainKind, ChainSpec, Expr, Hook, IfName, TableSpec, CT_ESTABLISHED_RELATED};
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
        Expr::L4(6) => "tcp".into(),
        Expr::L4(17) => "udp".into(),
        Expr::L4(n) => format!("proto {n}"),
        Expr::Sport(p) => format!("sport {p}"),
        Expr::Dport(p) => format!("dport {p}"),
        Expr::MarkIs(m) => format!("mark {m:#x}"),
        Expr::Accept => "accept".into(),
        Expr::Drop => "drop".into(),
        Expr::Return => "return".into(),
        Expr::RedirectPort(p) => format!("redirect {p}"),
        Expr::Masquerade => "masquerade".into(),
    }
}

/// The table as text, one rule per line (what `nft list table` would show, for the golden test and for support).
pub fn describe(t: &TableSpec) -> String {
    let mut s = format!("table inet {}\n", t.name);
    for c in &t.chains {
        let hook = match c.hook {
            Hook::Input => "input",
            Hook::Forward => "forward",
            Hook::Output => "output",
            Hook::Postrouting => "postrouting",
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
