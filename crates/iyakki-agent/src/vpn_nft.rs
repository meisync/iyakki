//! The VPN guard and the DNS redirect as typed tables for the Firewall's own netlink writer
//! ([`iyakki_fw::nl::tablegen`]). The images ship no `nft` program, so this is the only way these tables reach the kernel.
//!
//! The conversion is pure. A test evaluates the converted rule lists with a tiny evaluator and compares every verdict with
//! [`KillSwitchSpec::evaluate`], so the typed rules mean what the plan says; real kernels are covered by the ignored tests in
//! `iyakki-fw/tests/tablegen_kernel.rs`.

use std::net::IpAddr;
use iyakki_fw::nl::tablegen::{ChainKind, ChainSpec, Expr, Hook, IfName, SetSpec, TableSpec};
use iyakki_vpn::plan::{DnsCaptureSpec, IfMatch, KillSwitchSpec, Proto, Verdict, DNS_CHAIN, DNS_PRIORITY, DNS_TABLE, KS_CHAIN, KS_PRIORITY, KS_TABLE};

/// First and last address of a prefix, as numbers (host bits of the stored address are ignored).
fn cidr_span(c: &iyakki_vpn::net::Cidr) -> (u128, u128) {
    let (bits, all): (u32, u128) = if c.is_v4() { (32, u128::from(u32::MAX)) } else { (128, u128::MAX) };
    let n = match c.addr {
        IpAddr::V4(a) => u128::from(u32::from(a)),
        IpAddr::V6(a) => u128::from(a),
    };
    let host = if u32::from(c.prefix) >= bits { 0 } else { all >> c.prefix };
    let lo = n & !host & all;
    (lo, lo | host)
}

fn num_ip(v4: bool, n: u128) -> IpAddr {
    if v4 {
        IpAddr::V4(std::net::Ipv4Addr::from(n as u32))
    } else {
        IpAddr::V6(std::net::Ipv6Addr::from(n))
    }
}

fn ifname(m: &IfMatch) -> IfName {
    match m {
        IfMatch::Exact(n) => IfName::Exact(n.clone()),
        IfMatch::Prefix(n) => IfName::Prefix(n.clone()),
    }
}

/// `inet iyakki_vpn`, chain `output` (hook output, priority -10, policy accept).
pub fn guard_table(spec: &KillSwitchSpec) -> Result<TableSpec, String> {
    let mut sets = Vec::new();
    for s in &spec.sets {
        let mut members = Vec::new();
        let mut spans: Vec<(u128, u128)> = Vec::new();
        for c in &s.members {
            if c.is_v4() != s.v4 {
                return Err(format!("the set {} holds one address family", s.name));
            }
            let (lo, hi) = cidr_span(c);
            spans.push((lo, hi));
            if lo == hi {
                members.push(c.addr);
            }
        }
        // A network prefix needs an interval set. Then every member goes in as a span, merged so none overlap (the kernel refuses overlaps).
        let ranges: Vec<(IpAddr, IpAddr)> = if spans.iter().any(|(lo, hi)| lo != hi) {
            members.clear();
            iyakki_fw::ruleset::merge_spans(spans).into_iter().map(|(lo, hi)| (num_ip(s.v4, lo), num_ip(s.v4, hi))).collect()
        } else {
            Vec::new()
        };
        sets.push(SetSpec { name: s.name.clone(), v4: s.v4, timeout: true, members, ranges, concat: Vec::new() });
    }
    let mut rules = Vec::new();
    for r in &spec.rules {
        let m = &r.m;
        let mut e = Vec::new();
        if let Some(o) = &m.oif {
            e.push(Expr::OifIs(ifname(o)));
        }
        if let Some(o) = &m.oif_not {
            e.push(Expr::OifNot(ifname(o)));
        }
        if let Some(d) = &m.daddr {
            e.push(Expr::Daddr(d.addr, d.prefix));
        }
        if let Some(n) = &m.daddr_set {
            e.push(Expr::DaddrSet(n.clone()));
        }
        if let Some(p) = m.proto {
            e.push(Expr::L4(match p {
                Proto::Tcp => 6,
                Proto::Udp => 17,
                Proto::Icmpv6 => 58,
            }));
        }
        if let Some(p) = m.sport {
            e.push(Expr::Sport(p));
        }
        if let Some(p) = m.dport {
            e.push(Expr::Dport(p));
        }
        e.push(if r.verdict == Verdict::Accept { Expr::Accept } else { Expr::Drop });
        rules.push(e);
    }
    let t = TableSpec { name: KS_TABLE.into(), sets, chains: vec![ChainSpec { name: KS_CHAIN.into(), kind: ChainKind::Filter, hook: Hook::Output, priority: KS_PRIORITY, policy_accept: true, rules }] };
    t.check()?;
    Ok(t)
}

/// `inet iyakki_vpn_dns`, chain `output` of type nat: the forwarder's own (marked) queries pass, port 53 goes to the forwarder.
pub fn dns_table(spec: &DnsCaptureSpec) -> Result<TableSpec, String> {
    let t = TableSpec {
        name: DNS_TABLE.into(),
        sets: vec![],
        chains: vec![ChainSpec {
            name: DNS_CHAIN.into(),
            kind: ChainKind::Nat,
            hook: Hook::Output, priority: DNS_PRIORITY,
            policy_accept: true,
            rules: vec![
                vec![Expr::MarkIs(spec.mark), Expr::Return],
                vec![Expr::L4(17), Expr::Dport(53), Expr::RedirectPort(spec.listen_port)],
                vec![Expr::L4(6), Expr::Dport(53), Expr::RedirectPort(spec.listen_port)],
            ],
        }],
    };
    t.check()?;
    Ok(t)
}

/// The marking table of policy routing (F-VPN-08): `inet iyakki_vpn_mark`, chain `output` of type route at the mangle priority.
/// The DNS forwarder's own queries pass first, then the excluded destinations, then the chosen destinations and accounts get the
/// plan's mark (the kernel routes the packet again with the new mark, so `ip rule fwmark ... lookup` sends it into the tunnel).
/// Address lists become interval sets (merged, so none overlap); the text form of the same plan is `PolicyPlan::nft`.
pub fn policy_table(plan: &iyakki_vpn::policyroute::PolicyPlan) -> Result<TableSpec, String> {
    use iyakki_vpn::policyroute::MARK_TABLE;
    let mut sets = Vec::new();
    let mut rules: Vec<Vec<Expr>> = vec![vec![Expr::MarkIs(iyakki_vpn::plan::DNS_MARK), Expr::Return]];
    let mut add = |name: &str, v4: bool, cs: &[iyakki_vpn::net::Cidr]| -> Option<String> {
        let spans: Vec<(u128, u128)> = cs.iter().filter(|c| c.is_v4() == v4).map(cidr_span).collect();
        if spans.is_empty() {
            return None;
        }
        let ranges = iyakki_fw::ruleset::merge_spans(spans).into_iter().map(|(lo, hi)| (num_ip(v4, lo), num_ip(v4, hi))).collect();
        sets.push(SetSpec { name: name.into(), v4, timeout: false, members: Vec::new(), ranges, concat: Vec::new() });
        Some(name.to_string())
    };
    let ex4 = add("never4", true, plan.exclude());
    let ex6 = add("never6", false, plan.exclude());
    let in4 = add("tunnel4", true, plan.include());
    let in6 = add("tunnel6", false, plan.include());
    for n in [ex4, ex6].into_iter().flatten() {
        rules.push(vec![Expr::DaddrSet(n), Expr::Return]);
    }
    for n in [in4, in6].into_iter().flatten() {
        rules.push(vec![Expr::DaddrSet(n), Expr::SetMark(plan.mark()), Expr::Accept]);
    }
    for u in plan.uids() {
        rules.push(vec![Expr::SkuidIs(*u), Expr::SetMark(plan.mark()), Expr::Accept]);
    }
    let t = TableSpec { name: MARK_TABLE.into(), sets, chains: vec![ChainSpec { name: "output".into(), kind: ChainKind::Route, hook: Hook::Output, priority: -150, policy_accept: true, rules }] };
    t.check()?;
    Ok(t)
}

/// What the kernel needs loaded for these tables (best effort; built-in options simply fail to load, which is fine).
pub const MODULES: [&str; 3] = ["nf_tables", "nft_chain_nat", "nft_redir"];

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_vpn::net::Cidr;
    use iyakki_vpn::plan::{self, FailClosedInput, GuardBase, KillSwitchInput, Packet};

    /// A packet as the evaluator sees it.
    struct Pkt<'a> {
        p: &'a Packet,
        members: &'a [(String, Vec<IpAddr>)],
    }

    /// First-match evaluation of a typed rule list, chain policy accept. Mirrors what the kernel does with the encoded steps.
    fn eval(t: &TableSpec, p: &Pkt) -> Verdict {
        'rules: for r in &t.chains[0].rules {
            let mut l4: Option<u8> = None;
            for e in r {
                let ok = match e {
                    Expr::OifIs(IfName::Exact(n)) => &p.p.oif == n,
                    Expr::OifIs(IfName::Prefix(n)) => p.p.oif.starts_with(n.as_str()),
                    Expr::OifNot(IfName::Exact(n)) => &p.p.oif != n,
                    Expr::OifNot(IfName::Prefix(n)) => !p.p.oif.starts_with(n.as_str()),
                    Expr::Daddr(ip, pre) => Cidr::new(*ip, *pre).unwrap().contains_ip(p.p.daddr) && ip.is_ipv4() == p.p.daddr.is_ipv4(),
                    Expr::DaddrSet(n) => {
                        let s = t.sets.iter().find(|s| &s.name == n).unwrap();
                        s.v4 == p.p.daddr.is_ipv4() && (p.members.iter().any(|(m, v)| m == n && v.contains(&p.p.daddr)) || s.ranges.iter().any(|(lo, hi)| *lo <= p.p.daddr && p.p.daddr <= *hi))
                    }
                    Expr::L4(x) => {
                        l4 = Some(*x);
                        *x == match p.p.proto {
                            Proto::Tcp => 6,
                            Proto::Udp => 17,
                            Proto::Icmpv6 => 58,
                        } && (*x != 58 || p.p.daddr.is_ipv6())
                    }
                    Expr::Sport(x) => *x == p.p.sport,
                    Expr::Dport(x) => *x == p.p.dport,
                    Expr::Accept => return Verdict::Accept,
                    Expr::Drop => return Verdict::Drop,
                    // the outgoing firewall's steps (ranges, reject, counter, dnat, ct status) are never in the VPN guard
                    other => unreachable!("not in a guard: {other:?}"),
                };
                let _ = l4;
                if !ok {
                    continue 'rules;
                }
            }
        }
        Verdict::Accept
    }

    fn base<'a>(eps: &'a [(IpAddr, u16, Proto)], phys: &'a [String], lan: &'a [Cidr], cluster: &'a [Cidr]) -> GuardBase<'a> {
        GuardBase { ifname: "iyvpn0", physical_ifs: phys, endpoints: eps, lan, cluster, cluster_if_prefixes: &["cni", "flannel", "veth"], dev_port: 7979 }
    }

    fn pk(oif: &str, ip: &str, proto: Proto, sport: u16, dport: u16) -> Packet {
        Packet { oif: oif.into(), daddr: ip.parse().unwrap(), proto, sport, dport }
    }

    fn grid() -> Vec<Packet> {
        let mut v = Vec::new();
        for oif in ["eth0", "iyvpn0", "lo", "cni0", "flannel.1"] {
            for ip in ["203.0.113.9", "198.51.100.7", "192.168.1.20", "10.42.0.5", "8.8.8.8", "fe80::1", "ff02::2", "2001:db8::7", "2606:4700::1111"] {
                for (proto, sp, dp) in [(Proto::Udp, 4000, 53), (Proto::Udp, 68, 67), (Proto::Tcp, 7979, 51000), (Proto::Tcp, 4000, 443), (Proto::Icmpv6, 0, 0), (Proto::Udp, 5000, 51820)] {
                    v.push(pk(oif, ip, proto, sp, dp));
                }
            }
        }
        v
    }

    #[test]
    fn the_converted_kill_switch_decides_exactly_like_the_plan() {
        let eps = [("198.51.100.7".parse().unwrap(), 51820, Proto::Udp)];
        let phys = ["eth0".to_string()];
        let lan = [Cidr::parse("192.168.1.0/24").unwrap()];
        let cluster = [Cidr::parse("10.42.0.0/16").unwrap()];
        let spec = plan::kill_switch(&KillSwitchInput { base: base(&eps, &phys, &lan, &cluster), block_dns_outside: true }).unwrap();
        let t = guard_table(&spec).unwrap();
        let no_members: Vec<(String, Vec<IpAddr>)> = vec![];
        let mut dropped = 0;
        for p in grid() {
            let want = spec.evaluate(&p);
            assert_eq!(eval(&t, &Pkt { p: &p, members: &no_members }), want, "{p:?}");
            dropped += (want == Verdict::Drop) as usize;
        }
        assert!(dropped > 20 && dropped < grid().len(), "the grid has both verdicts");
    }

    #[test]
    fn the_converted_fail_closed_guard_decides_like_the_plan_including_the_learned_sets() {
        let eps = [("198.51.100.7".parse().unwrap(), 51820, Proto::Udp)];
        let phys = ["eth0".to_string()];
        let lan = [Cidr::parse("192.168.1.0/24").unwrap()];
        let cluster = [Cidr::parse("10.42.0.0/16").unwrap()];
        let nets = [(Cidr::parse("203.0.113.0/24").unwrap(), true), (Cidr::parse("203.0.113.128/25").unwrap(), false)];
        let spec = plan::fail_closed_guard(&FailClosedInput { base: base(&eps, &phys, &lan, &cluster), nets: &nets }).unwrap();
        let learned4 = Cidr::parse("8.8.8.8/32").unwrap();
        let learned6 = Cidr::parse("2606:4700::1111/128").unwrap();
        let with = spec.with_members("vpn_dest4", vec![learned4]).with_members("vpn_dest6", vec![learned6]);
        let t = guard_table(&spec).unwrap();
        assert!(t.sets.iter().all(|s| s.timeout && s.members.is_empty()));
        let members = vec![("vpn_dest4".to_string(), vec![learned4.addr]), ("vpn_dest6".to_string(), vec![learned6.addr])];
        let mut dropped = 0;
        for p in grid() {
            let want = with.evaluate(&p);
            assert_eq!(eval(&t, &Pkt { p: &p, members: &members }), want, "{p:?}");
            dropped += (want == Verdict::Drop) as usize;
        }
        assert!(dropped > 5, "the learned addresses and the included range are dropped off the tunnel");
        // static members are carried when they are single hosts and refused otherwise
        let mut s2 = spec.clone();
        s2.sets[0].members = vec![Cidr::parse("8.8.4.4/32").unwrap()];
        assert_eq!(guard_table(&s2).unwrap().sets[0].members.len(), 1);
        // a network prefix makes an interval set: the single address and the prefix are merged into spans
        s2.sets[0].members = vec![Cidr::parse("8.8.4.0/24").unwrap(), Cidr::parse("8.8.4.7/32").unwrap(), Cidr::parse("8.8.5.0/24").unwrap()];
        let t2 = guard_table(&s2).unwrap();
        assert!(t2.sets[0].members.is_empty());
        assert_eq!(t2.sets[0].ranges, vec![("8.8.4.0".parse().unwrap(), "8.8.5.255".parse().unwrap())]);
        let (bytes, _) = iyakki_fw::nl::tablegen::encode_apply(&t2, 1).unwrap();
        assert!(bytes.windows(8).any(|w| w == [8, 0, 3, 0, 0, 0, 0, 0x14]), "timeout flag 0x10 and interval flag 0x4 on the set");
        // the set holds one family only
        s2.sets[0].members = vec![Cidr::parse("2001:db8::/32").unwrap()];
        assert!(guard_table(&s2).is_err());
    }

    #[test]
    fn the_dns_redirect_table_matches_the_plan_text() {
        let spec = plan::dns_capture(15353).unwrap();
        let t = dns_table(&spec).unwrap();
        assert_eq!(t.name, "iyakki_vpn_dns");
        assert_eq!(t.chains[0].kind, ChainKind::Nat);
        assert_eq!(t.chains[0].priority, -100);
        let r = &t.chains[0].rules;
        assert_eq!(r[0], vec![Expr::MarkIs(spec.mark), Expr::Return]);
        assert_eq!(r[1], vec![Expr::L4(17), Expr::Dport(53), Expr::RedirectPort(15353)]);
        assert_eq!(r[2], vec![Expr::L4(6), Expr::Dport(53), Expr::RedirectPort(15353)]);
        // both bytes of the spec and the nft text agree on the numbers
        let text = spec.render();
        assert!(text.contains("redirect to :15353") && text.contains("priority -100"));
        // and the whole thing encodes
        assert!(iyakki_fw::nl::tablegen::encode_apply(&t, 1).is_ok());
    }

    #[test]
    fn vpn08_the_policy_table_marks_chosen_destinations_and_accounts_after_the_excluded_ones() {
        use iyakki_vpn::policyroute::{plan_policy_routes, PolicyInput};
        let (inc, exc) = (vec![Cidr::parse("198.51.100.0/24").unwrap(), Cidr::parse("2001:db8::/32").unwrap()], vec![Cidr::parse("198.51.100.7/32").unwrap()]);
        let plan = plan_policy_routes(&PolicyInput { ifname: "iyvpn0", table: 0x4956, mark: 0x4956_0002, priority: 10_000, include: &inc, exclude: &exc, uids: &[4242] }).unwrap();
        let t = policy_table(&plan).unwrap();
        let c = &t.chains[0];
        assert_eq!(c.kind, ChainKind::Route);
        assert_eq!(c.rules[0], vec![Expr::MarkIs(iyakki_vpn::plan::DNS_MARK), Expr::Return], "the forwarder's own queries first");
        let pos = |name: &str| c.rules.iter().position(|r| r.contains(&Expr::DaddrSet(name.into()))).unwrap();
        assert!(pos("never4") < pos("tunnel4"), "exclusions before inclusions");
        assert!(c.rules.iter().any(|r| r == &vec![Expr::SkuidIs(4242), Expr::SetMark(0x4956_0002), Expr::Accept]));
        assert!(t.sets.iter().any(|s| s.name == "tunnel6" && !s.v4));
        assert!(iyakki_fw::nl::tablegen::encode_apply_in(&t, iyakki_fw::nl::tablegen::Family::Inet, 1).is_ok(), "the writer encodes it");
    }
}
