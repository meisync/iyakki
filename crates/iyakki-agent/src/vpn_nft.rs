//! The VPN guard and the DNS redirect as typed tables for the Firewall's own netlink writer
//! ([`iyakki_fw::nl::tablegen`]). The images ship no `nft` program, so this is the only way these tables reach the kernel.
//!
//! The conversion is pure. A test evaluates the converted rule lists with a tiny evaluator and compares every verdict with
//! [`KillSwitchSpec::evaluate`], so the typed rules mean what the plan says; real kernels are covered by the ignored tests in
//! `iyakki-fw/tests/tablegen_kernel.rs`.

use iyakki_fw::nl::tablegen::{ChainKind, ChainSpec, Expr, Hook, IfName, SetSpec, TableSpec};
use iyakki_vpn::plan::{DnsCaptureSpec, IfMatch, KillSwitchSpec, Proto, Verdict, DNS_CHAIN, DNS_PRIORITY, DNS_TABLE, KS_CHAIN, KS_PRIORITY, KS_TABLE};

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
        for c in &s.members {
            if c.prefix != if c.is_v4() { 32 } else { 128 } {
                return Err(format!("the set {} takes single addresses only", s.name));
            }
            members.push(c.addr);
        }
        sets.push(SetSpec { name: s.name.clone(), v4: s.v4, timeout: true, members });
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

/// What the kernel needs loaded for these tables (best effort; built-in options simply fail to load, which is fine).
pub const MODULES: [&str; 3] = ["nf_tables", "nft_chain_nat", "nft_redir"];

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_vpn::net::Cidr;
    use iyakki_vpn::plan::{self, FailClosedInput, GuardBase, KillSwitchInput, Packet};
    use std::net::IpAddr;

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
                        s.v4 == p.p.daddr.is_ipv4() && p.members.iter().any(|(m, v)| m == n && v.contains(&p.p.daddr))
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
                    Expr::MarkIs(_) | Expr::RedirectPort(_) | Expr::Return | Expr::IifIs(_) | Expr::IifNot(_) | Expr::CtState(_) | Expr::Saddr(..) | Expr::Masquerade => unreachable!("not in a guard"),
                    Expr::Accept => return Verdict::Accept,
                    Expr::Drop => return Verdict::Drop,
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
        s2.sets[0].members = vec![Cidr::parse("8.8.4.0/24").unwrap()];
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
}
