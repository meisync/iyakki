//! Tests for the DNS wire reader and the forwarder state machine. Addresses are documentation ranges; no sockets, no clock.

use std::net::IpAddr;

use crate::dnsroute::*;
use crate::dnswire::*;
use crate::filter::FilterSet;
use crate::net::{default_cluster_cidrs, Cidr, Context};

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}
fn ctx() -> Context {
    Context { cluster: default_cluster_cidrs(), lan: vec![Cidr::parse("192.168.1.0/24").unwrap()], tunnel: vec![Cidr::parse("10.8.0.2/24").unwrap()] }
}
fn fw(fail_closed: bool, filters: &str) -> Forwarder {
    let mut f = Forwarder::new(Config::new(fail_closed, vec![ip("203.0.113.9")], ctx()), FilterSet::parse(filters).unwrap());
    f.handle(Event::TunnelState(true));
    f
}
const FILTERS: &str = "*.corp.example.com\ncorp.example.com\n!public.corp.example.com\n198.51.100.0/24\n!198.51.100.99/32";

fn ask(f: &mut Forwarder, client: u64, name: &str, id: u16, up: u16, now: u64) -> (Vec<u8>, Vec<Action>) {
    let q = build_query(id, name, TYPE_A);
    let a = f.handle(Event::Query { client, packet: q.clone(), upstream_id: up, now });
    (q, a)
}
fn sent(a: &[Action]) -> Vec<(Upstream, Vec<u8>)> {
    a.iter().filter_map(|x| if let Action::Send { to, packet } = x { Some((*to, packet.clone())) } else { None }).collect()
}
fn responded(a: &[Action]) -> Vec<(u64, Vec<u8>)> {
    a.iter().filter_map(|x| if let Action::Respond { client, packet } = x { Some((*client, packet.clone())) } else { None }).collect()
}
fn rcode(p: &[u8]) -> u8 {
    p[3] & 0xf
}

#[test]
fn wire_round_trip_and_hostile_packets() {
    let q = build_query(0x1234, "WWW.Example.COM", TYPE_A);
    let (id, qu) = parse_query(&q).unwrap();
    assert_eq!((id, qu.name.as_str(), qu.qtype, qu.qclass), (0x1234, "www.example.com", 1, 1));
    let r = build_response(&q, 0, &[(ip("192.0.2.1"), 60), (ip("2001:db8::1"), 120)]).unwrap();
    let p = parse_response(&r).unwrap();
    assert_eq!((p.id, p.rcode, p.answers.len()), (0x1234, 0, 2));
    assert_eq!(p.answers[0].ip, ip("192.0.2.1"));
    assert_eq!(p.answers[1].ttl, 120);
    assert_eq!(rcode(&error_response(&q, RCODE_REFUSED).unwrap()), 5);
    assert!(parse_response(&q).is_err(), "a query is not a response");
    assert!(parse_query(&r).is_err(), "a response is not a query");
    // every prefix and every flipped byte of a valid packet is an error or a parse, never a panic
    for cut in 0..r.len() {
        let _ = parse_response(&r[..cut]);
        let _ = parse_query(&r[..cut]);
    }
    for i in 0..r.len() {
        let mut m = r.clone();
        m[i] ^= 0xff;
        let _ = parse_response(&m);
        let _ = error_response(&m, 2);
    }
    // a compression loop, a pointer past the end, a forward pointer, an oversized count, a huge packet
    let mut lp = build_query(1, "a.com", TYPE_A);
    lp[12] = 0xc0;
    lp[13] = 12;
    assert!(matches!(parse_query(&lp), Err(DnsError::Malformed(_))));
    let mut far = lp.clone();
    far[13] = 0xff;
    assert!(parse_query(&far).is_err());
    let mut many = build_response(&q, 0, &[(ip("192.0.2.1"), 60)]).unwrap();
    many[6] = 0xff;
    many[7] = 0xff;
    assert!(parse_response(&many).is_err());
    assert_eq!(parse_query(&vec![0; MAX_PACKET + 1]), Err(DnsError::TooBig));
    assert_eq!(parse_query(&[0; 5]), Err(DnsError::Short));
    // two questions, or an answer in a query
    let mut two = q.clone();
    two[5] = 2;
    assert!(parse_query(&two).is_err());
    // odd bytes in a name become '?' and match nothing
    let mut odd = build_query(2, "ab.com", TYPE_A);
    odd[13] = b'$';
    assert_eq!(parse_query(&odd).unwrap().1.name, "?b.com");
    // a wrong-sized address record is malformed
    let mut bad = build_response(&q, 0, &[(ip("192.0.2.1"), 60)]).unwrap();
    let n = bad.len();
    bad[n - 5] = 5;
    assert!(parse_response(&bad).is_err());
}

#[test]
fn a_tunnel_name_goes_to_the_vpn_dns_and_its_addresses_become_routes_before_the_answer_is_returned() {
    let mut f = fw(true, FILTERS);
    let (q, a) = ask(&mut f, 7, "App.Corp.Example.com", 0xAAAA, 0x1111, 1000);
    let s = sent(&a);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].0, Upstream::Vpn);
    assert_eq!(&s[0].1[0..2], &0x1111u16.to_be_bytes(), "the id is replaced by the one the executor chose");
    assert_eq!(&s[0].1[2..], &q[2..]);
    let reply = build_response(&s[0].1, 0, &[(ip("192.0.2.10"), 300), (ip("2001:db8::10"), 5)]).unwrap();
    let out = f.handle(Event::Reply { from: Upstream::Vpn, packet: reply.clone(), now: 1001 });
    let kinds: Vec<&str> = out.iter().map(|a| match a { Action::RouteAdd { .. } => "add", Action::Respond { .. } => "respond", _ => "other" }).collect();
    assert_eq!(kinds, ["add", "add", "respond"], "routes first, answer last");
    assert_eq!(out[0], Action::RouteAdd { ip: ip("192.0.2.10"), expires: 1001 + 300 + 30 });
    assert_eq!(out[1], Action::RouteAdd { ip: ip("2001:db8::10"), expires: 1001 + 30 + 30 }, "a short TTL is raised to the minimum");
    let r = responded(&out);
    assert_eq!(r[0].0, 7);
    assert_eq!(&r[0].1[0..2], &0xAAAAu16.to_be_bytes(), "the program sees its own id again");
    assert_eq!(&r[0].1[2..], &reply[2..]);
    assert_eq!(f.routes().len(), 2);
    assert_eq!(f.pending_count(), 0);
}

#[test]
fn other_names_go_to_the_system_resolver_untouched_and_get_no_routes() {
    let mut f = fw(true, FILTERS);
    for name in ["www.example.org", "public.corp.example.com", "corp.example.com.evil.net", "example.com"] {
        let (_, a) = ask(&mut f, 1, name, 5, 6, 10);
        let s = sent(&a);
        assert_eq!(s[0].0, if name == "public.corp.example.com" || !name.contains("corp") || name.ends_with("evil.net") { Upstream::System } else { Upstream::Vpn }, "{name}");
    }
    let (_, a) = ask(&mut f, 1, "www.example.org", 9, 0x2222, 10);
    let reply = build_response(&sent(&a)[0].1, 0, &[(ip("192.0.2.55"), 300)]).unwrap();
    let out = f.handle(Event::Reply { from: Upstream::System, packet: reply, now: 11 });
    assert!(out.iter().all(|x| !matches!(x, Action::RouteAdd { .. })));
    assert_eq!(responded(&out).len(), 1);
    assert!(f.routes().is_empty());
}

#[test]
fn tunnel_down_fail_closed_refuses_and_never_asks_anyone_but_fail_open_goes_direct() {
    let mut f = fw(true, FILTERS);
    f.handle(Event::TunnelState(false));
    let (_, a) = ask(&mut f, 3, "app.corp.example.com", 1, 2, 5);
    assert!(sent(&a).is_empty(), "the name is not sent to the system resolver either");
    let r = responded(&a);
    assert_eq!((r[0].0, rcode(&r[0].1)), (3, RCODE_REFUSED));
    // a name that is not a tunnel name still resolves
    let (_, a) = ask(&mut f, 3, "www.example.org", 1, 3, 5);
    assert_eq!(sent(&a)[0].0, Upstream::System);

    let mut open = fw(false, FILTERS);
    open.handle(Event::TunnelState(false));
    let (_, a) = ask(&mut open, 3, "app.corp.example.com", 1, 2, 5);
    assert_eq!(sent(&a)[0].0, Upstream::System, "fail-open: direct");
}

#[test]
fn losing_the_tunnel_drops_the_routes_and_refuses_the_open_tunnel_queries() {
    let mut f = fw(true, FILTERS);
    let (_, a) = ask(&mut f, 1, "a.corp.example.com", 1, 100, 0);
    let ans = build_response(&sent(&a)[0].1, 0, &[(ip("192.0.2.1"), 300)]).unwrap();
    f.handle(Event::Reply { from: Upstream::Vpn, packet: ans, now: 1 });
    let (_, _b) = ask(&mut f, 2, "b.corp.example.com", 2, 101, 2);
    assert_eq!((f.routes().len(), f.pending_count()), (1, 1));
    let out = f.handle(Event::TunnelState(false));
    assert!(out.contains(&Action::RouteDel { ip: ip("192.0.2.1") }));
    let r = responded(&out);
    assert_eq!((r.len(), r[0].0, rcode(&r[0].1)), (1, 2, RCODE_REFUSED));
    assert!(f.routes().is_empty() && f.pending_count() == 0 && !f.tunnel_up());
}

#[test]
fn rebinding_and_hijack_answers_are_refused_and_no_route_is_made() {
    for bad in [
        "10.42.0.5", "10.43.0.1", "127.0.0.1", "169.254.169.254", "192.168.1.1", "192.168.1.200", "172.20.0.1", "10.9.9.9", "203.0.113.9", "0.0.0.0", "224.0.0.1", "::1", "fe80::1", "::ffff:10.42.0.5", "::ffff:127.0.0.1", "fd00::1",
    ] {
        let mut f = fw(true, FILTERS);
        let (_, a) = ask(&mut f, 1, "x.corp.example.com", 1, 2, 0);
        // a good address next to the bad one must not slip through either
        let ans = build_response(&sent(&a)[0].1, 0, &[(ip("192.0.2.1"), 300), (ip(bad), 300)]).unwrap();
        let out = f.handle(Event::Reply { from: Upstream::Vpn, packet: ans, now: 1 });
        assert!(out.iter().all(|x| !matches!(x, Action::RouteAdd { .. })), "{bad}");
        let r = responded(&out);
        assert_eq!((r.len(), rcode(&r[0].1)), (1, RCODE_SERVFAIL), "{bad}");
        assert!(f.routes().is_empty(), "{bad}");
    }
    // the VPN's own network is fine
    let mut f = fw(true, FILTERS);
    let (_, a) = ask(&mut f, 1, "x.corp.example.com", 1, 2, 0);
    let ans = build_response(&sent(&a)[0].1, 0, &[(ip("10.8.0.44"), 300)]).unwrap();
    let out = f.handle(Event::Reply { from: Upstream::Vpn, packet: ans, now: 1 });
    assert!(out.iter().any(|x| matches!(x, Action::RouteAdd { .. })));
}

#[test]
fn an_address_excluded_by_an_address_rule_gets_no_route() {
    let mut f = fw(true, FILTERS);
    let (_, a) = ask(&mut f, 1, "x.corp.example.com", 1, 2, 0);
    let ans = build_response(&sent(&a)[0].1, 0, &[(ip("198.51.100.99"), 300), (ip("198.51.100.5"), 300)]).unwrap();
    let out = f.handle(Event::Reply { from: Upstream::Vpn, packet: ans, now: 1 });
    let added: Vec<IpAddr> = out.iter().filter_map(|x| if let Action::RouteAdd { ip, .. } = x { Some(*ip) } else { None }).collect();
    assert_eq!(added, [ip("198.51.100.5")]);
    assert_eq!(responded(&out).len(), 1);
}

#[test]
fn replies_that_do_not_belong_are_ignored_and_the_query_stays_open() {
    let mut f = fw(true, FILTERS);
    let (_, a) = ask(&mut f, 1, "x.corp.example.com", 0x0101, 0x3333, 0);
    let out_q = sent(&a)[0].1.clone();
    let good = build_response(&out_q, 0, &[(ip("192.0.2.1"), 300)]).unwrap();
    // wrong upstream, wrong id, wrong question, wrong type, garbage, a query instead of a reply
    let wrong_id = crate::dnswire::with_id(&good, 0x3334);
    let other_q = build_response(&crate::dnswire::with_id(&build_query(0, "y.corp.example.com", TYPE_A), 0x3333), 0, &[(ip("192.0.2.1"), 300)]).unwrap();
    let other_t = build_response(&crate::dnswire::with_id(&build_query(0, "x.corp.example.com", TYPE_AAAA), 0x3333), 0, &[(ip("2001:db8::1"), 300)]).unwrap();
    for (from, p) in [(Upstream::System, good.clone()), (Upstream::Vpn, wrong_id), (Upstream::Vpn, other_q), (Upstream::Vpn, other_t), (Upstream::Vpn, vec![1, 2, 3]), (Upstream::Vpn, out_q.clone())] {
        let out = f.handle(Event::Reply { from, packet: p, now: 1 });
        assert!(out.iter().all(|x| matches!(x, Action::Note(_))), "{out:?}");
    }
    assert_eq!(f.pending_count(), 1);
    assert!(f.routes().is_empty());
    let out = f.handle(Event::Reply { from: Upstream::Vpn, packet: good, now: 2 });
    assert_eq!(responded(&out).len(), 1);
    // the same reply again finds no open query
    assert!(f.handle(Event::Reply { from: Upstream::Vpn, packet: build_response(&out_q, 0, &[(ip("192.0.2.1"), 300)]).unwrap(), now: 3 }).iter().all(|x| matches!(x, Action::Note(_))));
}

#[test]
fn routes_expire_and_a_longer_ttl_extends_them() {
    let mut f = fw(true, FILTERS);
    let learn = |f: &mut Forwarder, ttl: u32, now: u64, id: u16| {
        let (_, a) = ask(f, 1, "x.corp.example.com", id, id + 1000, now);
        let ans = build_response(&sent(&a)[0].1, 0, &[(ip("192.0.2.1"), ttl)]).unwrap();
        f.handle(Event::Reply { from: Upstream::Vpn, packet: ans, now })
    };
    let first = learn(&mut f, 100, 1000, 1);
    assert!(first.contains(&Action::RouteAdd { ip: ip("192.0.2.1"), expires: 1130 }));
    // the same address with a shorter lifetime changes nothing
    assert!(learn(&mut f, 40, 1010, 2).iter().all(|x| !matches!(x, Action::RouteAdd { .. })));
    // with a longer one the route is extended
    assert!(learn(&mut f, 600, 1020, 3).contains(&Action::RouteAdd { ip: ip("192.0.2.1"), expires: 1020 + 630 }));
    assert!(f.handle(Event::Tick { now: 1649 }).is_empty());
    assert_eq!(f.handle(Event::Tick { now: 1650 }), vec![Action::RouteDel { ip: ip("192.0.2.1") }]);
    assert!(f.routes().is_empty());
    // a huge TTL is capped
    let out = learn(&mut f, u32::MAX, 2000, 4);
    assert!(out.contains(&Action::RouteAdd { ip: ip("192.0.2.1"), expires: 2000 + 3600 + 30 }));
}

#[test]
fn the_number_of_routes_and_open_queries_is_capped() {
    let mut cfg = Config::new(true, vec![], ctx());
    cfg.max_routes = 3;
    cfg.max_pending = 4;
    let mut f = Forwarder::new(cfg, FilterSet::parse(FILTERS).unwrap());
    f.handle(Event::TunnelState(true));
    for n in 1..=5u8 {
        let (_, a) = ask(&mut f, 1, "x.corp.example.com", n as u16, 100 + n as u16, n as u64);
        let ans = build_response(&sent(&a)[0].1, 0, &[(ip(&format!("192.0.2.{n}")), 100)]).unwrap();
        let out = f.handle(Event::Reply { from: Upstream::Vpn, packet: ans, now: n as u64 });
        if n > 3 {
            // the route that expires first makes room, and it is removed before the new one is added
            let del = out.iter().position(|x| matches!(x, Action::RouteDel { .. })).unwrap();
            let add = out.iter().position(|x| matches!(x, Action::RouteAdd { .. })).unwrap();
            assert!(del < add);
        }
        assert!(f.routes().len() <= 3);
    }
    assert_eq!(f.routes().len(), 3);
    assert!(!f.routes().iter().any(|(a, _)| *a == ip("192.0.2.1")), "the oldest was evicted");
    // open queries
    for n in 0..4u16 {
        let (_, a) = ask(&mut f, 9, "y.corp.example.com", n, 500 + n, 100);
        assert_eq!(sent(&a).len(), 1);
    }
    let (_, a) = ask(&mut f, 9, "z.corp.example.com", 99, 600, 100);
    assert!(sent(&a).is_empty());
    assert_eq!(rcode(&responded(&a)[0].1), RCODE_SERVFAIL);
    // queries that never get a reply time out with SERVFAIL
    let out = f.handle(Event::Tick { now: 106 });
    let r = responded(&out);
    assert_eq!(r.len(), 4);
    assert!(r.iter().all(|(_, p)| rcode(p) == RCODE_SERVFAIL));
    assert_eq!(f.pending_count(), 0);
}

#[test]
fn too_many_addresses_in_one_answer_are_refused_not_partly_routed() {
    let mut f = fw(true, FILTERS);
    let (_, a) = ask(&mut f, 1, "x.corp.example.com", 1, 2, 0);
    let many: Vec<(IpAddr, u32)> = (1..=(MAX_ADDRS_PER_ANSWER as u8 + 1)).map(|n| (ip(&format!("192.0.2.{n}")), 60)).collect();
    let out = f.handle(Event::Reply { from: Upstream::Vpn, packet: build_response(&sent(&a)[0].1, 0, &many).unwrap(), now: 1 });
    assert!(out.iter().all(|x| !matches!(x, Action::RouteAdd { .. })));
    assert_eq!(rcode(&responded(&out)[0].1), RCODE_SERVFAIL);
    // exactly the limit is fine
    let (_, a) = ask(&mut f, 1, "x.corp.example.com", 3, 4, 0);
    let ok: Vec<(IpAddr, u32)> = many[..MAX_ADDRS_PER_ANSWER].to_vec();
    let out = f.handle(Event::Reply { from: Upstream::Vpn, packet: build_response(&sent(&a)[0].1, 0, &ok).unwrap(), now: 1 });
    assert_eq!(out.iter().filter(|x| matches!(x, Action::RouteAdd { .. })).count(), MAX_ADDRS_PER_ANSWER);
}

#[test]
fn error_and_empty_answers_are_relayed_without_routes_and_junk_queries_are_dropped() {
    let mut f = fw(true, FILTERS);
    let (_, a) = ask(&mut f, 1, "x.corp.example.com", 1, 2, 0);
    let nx = build_response(&sent(&a)[0].1, 3, &[]).unwrap();
    let out = f.handle(Event::Reply { from: Upstream::Vpn, packet: nx, now: 1 });
    assert_eq!(out.len(), 1);
    assert_eq!(rcode(&responded(&out)[0].1), 3);
    for junk in [vec![], vec![1, 2, 3], vec![0xff; 600], build_response(&build_query(1, "a.com", TYPE_A), 0, &[]).unwrap()] {
        let out = f.handle(Event::Query { client: 1, packet: junk, upstream_id: 1, now: 0 });
        assert!(out.iter().all(|x| matches!(x, Action::Note(_))));
    }
    assert_eq!(f.pending_count(), 0);
}

#[test]
fn colliding_upstream_ids_are_made_unique() {
    let mut f = fw(true, FILTERS);
    let (_, a) = ask(&mut f, 1, "a.corp.example.com", 1, 0x4242, 0);
    let (_, b) = ask(&mut f, 2, "b.corp.example.com", 1, 0x4242, 0);
    assert_eq!(&sent(&a)[0].1[0..2], &0x4242u16.to_be_bytes());
    assert_eq!(&sent(&b)[0].1[0..2], &0x4243u16.to_be_bytes());
    let out = f.handle(Event::Reply { from: Upstream::Vpn, packet: build_response(&sent(&b)[0].1, 0, &[(ip("192.0.2.2"), 60)]).unwrap(), now: 1 });
    assert_eq!(responded(&out)[0].0, 2);
}

#[test]
fn no_filters_means_nothing_goes_through_the_tunnel() {
    let mut f = fw(true, "");
    let (_, a) = ask(&mut f, 1, "anything.example.com", 1, 2, 0);
    assert_eq!(sent(&a)[0].0, Upstream::System);
}
