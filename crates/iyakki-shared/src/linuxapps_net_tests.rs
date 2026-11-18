use super::*;

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn outbound(allow: &[&str]) -> NetPolicy {
    NetPolicy { mode: Mode::OutboundOnly, allow: NetPolicy::parse_allow(&allow.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap(), ..Default::default() }
}

const HOST: [&str; 2] = ["192.168.1.20", "fd00::20"];

fn host() -> Vec<IpAddr> {
    HOST.iter().map(|s| ip(s)).collect()
}

#[test]
fn allow_entries_parse_in_every_form_and_bad_ones_say_why() {
    assert_eq!(Allow::parse("203.0.113.7:443").unwrap(), Allow { target: Target::Net { addr: ip("203.0.113.7"), prefix: 32 }, port: Some(443) });
    assert_eq!(Allow::parse("203.0.113.77/24").unwrap().target, Target::Net { addr: ip("203.0.113.0"), prefix: 24 }, "the host bits are cleared");
    assert_eq!(Allow::parse("Example.ORG:8443").unwrap(), Allow { target: Target::Name("example.org".into()), port: Some(8443) });
    assert_eq!(Allow::parse("[2001:db8::1]:443").unwrap().port, Some(443));
    assert_eq!(Allow::parse("2001:db8::/32").unwrap().target, Target::Net { addr: ip("2001:db8::"), prefix: 32 });
    assert_eq!(Allow::parse("2001:db8::5").unwrap().port, None);
    for bad in ["", "localhost", "a b", "example.org:0", "example.org:99999", "10.0.0.0/33", "0.0.0.0/0", "-x.org", "1.2.3", "ex_ample.org"] {
        assert!(Allow::parse(bad).is_err(), "{bad} should be refused");
    }
    // duplicates collapse, the text round-trips
    let l = NetPolicy::parse_allow(&["example.org:443".into(), "example.org:443".into(), "10.0.0.0/8".into()]).unwrap();
    assert_eq!(l.len(), 2);
    for a in &l {
        assert_eq!(&Allow::parse(&a.text()).unwrap(), a);
    }
    assert_eq!(Allow::parse("[2001:db8::1]:443").unwrap().text(), "[2001:db8::1]:443");
}

#[test]
fn the_host_and_its_neighbourhood_are_never_reachable() {
    let p = outbound(&[]);
    for dest in ["127.0.0.1", "127.8.8.8", "0.0.0.0", "169.254.169.254", "224.0.0.1", "255.255.255.255", "::1", "fe80::1", "ff02::1", "192.168.1.20", "fd00::20"] {
        assert_eq!(decide(&p, &host(), &[], ip(dest), 443), Err(Deny::Host), "{dest}");
    }
    // even when the allow-list names them
    let named = outbound(&["127.0.0.1", "192.168.1.20", "169.254.169.254"]);
    for dest in ["127.0.0.1", "192.168.1.20", "169.254.169.254"] {
        assert_eq!(decide(&named, &host(), &[], ip(dest), 80), Err(Deny::Host), "{dest}");
    }
}

#[test]
fn an_empty_list_means_any_public_address_but_no_private_network() {
    let p = outbound(&[]);
    assert_eq!(decide(&p, &host(), &[], ip("93.184.216.34"), 443), Ok(()));
    assert_eq!(decide(&p, &host(), &[], ip("2606:2800:220:1::1"), 443), Ok(()));
    for dest in ["10.1.2.3", "172.16.0.9", "192.168.1.99", "100.64.0.5", "fd12::1"] {
        assert_eq!(decide(&p, &host(), &[], ip(dest), 443), Err(Deny::Private), "{dest}");
    }
    assert!(p.reaches_any_server());
}

#[test]
fn a_list_allows_only_what_it_names_and_may_open_a_private_host_on_purpose() {
    let r = vec![("pkg.example.org".to_string(), vec![ip("198.51.100.10")])];
    let p = outbound(&["pkg.example.org:443", "192.168.1.50:8080", "203.0.113.0/24"]);
    assert!(!p.reaches_any_server());
    assert_eq!(decide(&p, &host(), &r, ip("198.51.100.10"), 443), Ok(()));
    assert_eq!(decide(&p, &host(), &r, ip("198.51.100.10"), 80), Err(Deny::NotListed), "wrong port");
    assert_eq!(decide(&p, &host(), &r, ip("198.51.100.11"), 443), Err(Deny::NotListed), "name resolved to another address");
    assert_eq!(decide(&p, &host(), &r, ip("192.168.1.50"), 8080), Ok(()), "a LAN machine the person listed");
    assert_eq!(decide(&p, &host(), &r, ip("192.168.1.51"), 8080), Err(Deny::Private));
    assert_eq!(decide(&p, &host(), &r, ip("203.0.113.200"), 22), Ok(()));
    assert_eq!(decide(&p, &host(), &r, ip("93.184.216.34"), 443), Err(Deny::NotListed));
}

#[test]
fn off_and_shared_modes_get_no_rule_set_and_shared_is_flagged() {
    for m in [Mode::Off, Mode::HostShared] {
        let p = NetPolicy { mode: m, ..Default::default() };
        assert!(table_spec(&p, &Veth::for_slot(1), &[], &[], &[]).is_err());
        assert_eq!(decide(&p, &[], &[], ip("93.184.216.34"), 443), Err(Deny::Host));
    }
    assert!(Mode::HostShared.reaches_host() && !Mode::OutboundOnly.reaches_host() && !Mode::Off.reaches_host());
    assert!(NetPolicy { mode: Mode::HostShared, ..Default::default() }.reaches_any_server());
    assert!(!NetPolicy::default().reaches_any_server());
    for m in [Mode::Off, Mode::HostShared, Mode::OutboundOnly] {
        assert_eq!(Mode::parse(m.id()), Some(m));
    }
    assert_eq!(Mode::parse("direct"), None);
}

/// Evaluate the forward chain for a new TCP/UDP connection from the sandbox, the way the kernel would: first matching rule wins.
fn verdict(spec: &TableSpec, veth: &Veth, dst: IpAddr, proto: u8, port: u16) -> bool {
    use iyakki_fw::nl::tablegen::{Expr, IfName};
    let chain = spec.chains.iter().find(|c| c.name == "forward").unwrap();
    for r in &chain.rules {
        let mut hit = true;
        for e in r {
            let ok = match e {
                Expr::IifIs(IfName::Exact(n)) => *n == veth.host_if,
                Expr::OifIs(IfName::Exact(_)) => false, // the packet leaves towards the internet
                Expr::CtState(_) => false,              // a new connection
                Expr::Daddr(net, p) => in_net(dst, *net, *p),
                Expr::L4(l) => *l == proto,
                Expr::Dport(d) => *d == port,
                Expr::Accept | Expr::Drop => true,
                other => panic!("unexpected {other:?}"),
            };
            if !ok {
                hit = false;
                break;
            }
        }
        if hit {
            return matches!(r.last(), Some(Expr::Accept));
        }
    }
    true // the chain's policy is accept: nothing in it matched
}

#[test]
fn the_typed_rules_give_the_same_verdicts_as_the_decision_function() {
    let v = Veth::for_slot(5);
    let r = vec![("pkg.example.org".to_string(), vec![ip("198.51.100.10"), ip("192.168.1.20")])];
    let dests = ["93.184.216.34", "2606:2800:220:1::1", "10.1.2.3", "172.20.0.1", "192.168.1.99", "192.168.1.50", "192.168.1.20", "100.64.0.5", "fd12::1", "127.0.0.1", "::1", "169.254.169.254", "fe80::1", "224.0.0.1", "198.51.100.10", "198.51.100.11", "203.0.113.200", "fd00::20"];
    for list in [&[][..], &["pkg.example.org:443", "192.168.1.50:8080", "203.0.113.0/24", "127.0.0.1"][..], &["2606:2800::/32"][..]] {
        let p = outbound(list);
        let spec = table_spec(&p, &v, &host(), &r, &[]).unwrap();
        for d in dests {
            for (proto, port) in [(6u8, 443u16), (6, 8080), (17, 443), (6, 80)] {
                let want = decide(&p, &host(), &r, ip(d), port).is_ok();
                assert_eq!(verdict(&spec, &v, ip(d), proto, port), want, "list {list:?}: {d} proto {proto} port {port}");
            }
        }
    }
}

#[test]
fn the_table_has_the_input_drop_the_masquerade_dns_before_the_private_block_and_no_catch_all_with_a_list() {
    use iyakki_fw::nl::tablegen::{Expr, Hook};
    let v = Veth::for_slot(5);
    let s = table_spec(&outbound(&[]), &v, &host(), &[], &[ip("192.168.1.1")]).unwrap();
    assert_eq!(s.name, "iyakki_sbx_0005");
    let input = s.chains.iter().find(|c| c.hook == Hook::Input).unwrap();
    assert_eq!(input.rules.len(), 1);
    assert!(matches!(input.rules[0].last(), Some(Expr::Drop)));
    let post = s.chains.iter().find(|c| c.hook == Hook::Postrouting).unwrap();
    assert!(post.rules[0].contains(&Expr::Masquerade) && post.rules[0].contains(&Expr::Saddr(ip("10.213.0.22"), 30)));
    // the nameserver on the private network can be asked for names although private networks are blocked
    assert!(verdict(&s, &v, ip("192.168.1.1"), 17, 53));
    assert!(!verdict(&s, &v, ip("192.168.1.1"), 6, 443));
    let list = table_spec(&outbound(&["example.org:443"]), &v, &host(), &[("example.org".into(), vec![])], &[]).unwrap();
    let fwd = list.chains.iter().find(|c| c.hook == Hook::Forward).unwrap();
    assert!(matches!(fwd.rules.last().unwrap().last(), Some(Expr::Drop)), "ends closed");
    assert!(!verdict(&list, &v, ip("93.184.216.34"), 6, 443), "a name that did not resolve allows nothing");
}

#[test]
fn slots_are_stable_distinct_and_the_addresses_fit_a_slash_30() {
    let a = pick_slot("debian", &[]).unwrap();
    assert_eq!(pick_slot("debian", &[]), Some(a), "stable");
    let b = pick_slot("debian", &[a]).unwrap();
    assert_ne!(a, b, "a taken slot is skipped");
    let all: Vec<u16> = (0..Veth::SLOTS as u16).collect();
    assert_eq!(pick_slot("x", &all), None);
    let v = Veth::for_slot(0);
    assert_eq!((v.host_ip, v.sandbox_ip, v.subnet().as_str()), (Ipv4Addr::new(10, 213, 0, 1), Ipv4Addr::new(10, 213, 0, 2), "10.213.0.0/30"));
    let last = Veth::for_slot(16383);
    assert_eq!((last.host_ip, last.subnet().as_str()), (Ipv4Addr::new(10, 213, 255, 253), "10.213.255.252/30"));
    let mut seen = std::collections::HashSet::new();
    for s in 0..Veth::SLOTS as u16 {
        let v = Veth::for_slot(s);
        assert!(v.host_if.len() <= 15 && v.sandbox_if.len() <= 15);
        seen.insert(v.subnet());
        seen.insert(v.host_if.clone());
    }
    assert_eq!(seen.len(), 2 * Veth::SLOTS as usize, "every slot has its own subnet and interface name");
}

#[test]
fn the_steps_are_exact_and_the_teardown_removes_what_was_made() {
    let v = Veth::for_slot(5);
    let up = setup_steps(&v, 4242);
    assert_eq!(up[0].argv.join(" "), "ip link add ivh0005 type veth peer name ivs0005");
    assert_eq!(up[1].argv.join(" "), "ip link set ivs0005 netns 4242");
    assert!(up.iter().filter(|s| !s.inside).count() == 4 && up.iter().filter(|s| s.inside).count() == 4);
    assert!(up.iter().any(|s| s.inside && s.argv.join(" ") == "ip route add default via 10.213.0.21"));
    assert!(up.iter().any(|s| s.inside && s.argv.join(" ") == "ip addr add 10.213.0.22/30 dev ivs0005"));
    assert!(up.iter().all(|s| s.argv[0] == "ip"), "only ip is needed");
    assert_eq!(teardown_steps(&v)[0].argv.join(" "), "ip link delete ivh0005");
    assert_eq!(table_name(&v), "iyakki_sbx_0005");
}

#[test]
fn the_computers_own_addresses_are_found_names_resolve_closed_and_hosts_are_gated() {
    assert!(local_addresses().iter().any(|a| a.is_loopback()), "the loopback address is there on any machine");
    let p = outbound(&["a.example.org:443", "b.example.org", "203.0.113.0/24"]);
    let r = resolve_names(&p, &|n| if n == "a.example.org" { vec![ip("198.51.100.1")] } else { vec![] });
    assert_eq!(r, vec![("a.example.org".to_string(), vec![ip("198.51.100.1")]), ("b.example.org".to_string(), vec![])]);
    let star = vec!["*".to_string()];
    let some = vec!["a.example.org".to_string()];
    assert!(host_gate(&p, &star).is_ok());
    assert!(host_gate(&p, &some).unwrap_err().contains("b.example.org"));
    assert!(host_gate(&outbound(&["a.example.org"]), &some).is_ok());
    assert!(host_gate(&outbound(&[]), &some).unwrap_err().contains("any public server"));
    assert!(host_gate(&outbound(&["203.0.113.0/24"]), &some).is_err(), "a network block needs every host");
    assert!(host_gate(&NetPolicy { mode: Mode::HostShared, ..Default::default() }, &some).is_err());
    assert!(host_gate(&NetPolicy::default(), &[]).is_ok(), "off needs nothing");
}

#[test]
fn a_loopback_stub_resolver_is_replaced_or_reported() {
    let stub = "# managed\nnameserver 127.0.0.53\noptions edns0\n";
    let err = resolv_for(stub, &NetPolicy::default()).unwrap_err();
    assert!(err.contains("local one"), "{err}");
    let mut p = NetPolicy::default();
    p.dns = vec![ip("9.9.9.9"), ip("127.0.0.1")];
    let (text, ns) = resolv_for(stub, &p).unwrap();
    assert_eq!((text.as_str(), ns), ("nameserver 9.9.9.9\n", vec![ip("9.9.9.9")]));
    let (text, _) = resolv_for("nameserver 192.168.1.1\nnameserver 127.0.0.1\nnameserver fe80::1%eth0\nnameserver 1.1.1.1\nnameserver 8.8.8.8\nnameserver 8.8.4.4\n", &p).unwrap();
    assert_eq!(text.lines().count(), 3, "at most three, host ones first: {text}");
    assert!(text.starts_with("nameserver 192.168.1.1\n"));
}

#[test]
fn the_policy_is_stored_beside_the_sandbox_and_fails_closed() {
    let dir = std::env::temp_dir().join(format!("iyakki-netpol-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("netpolicy.json");
    assert_eq!(NetPolicy::load(&f).mode, Mode::Off, "missing file");
    let p = outbound(&["pkg.example.org:443", "203.0.113.0/24"]);
    p.save(&f).unwrap();
    assert_eq!(NetPolicy::load(&f), p);
    assert!(!dir.join("netpolicy.json.tmp").exists());
    std::fs::write(&f, "{ not json").unwrap();
    assert_eq!(NetPolicy::load(&f).mode, Mode::Off, "damaged file");
    std::fs::write(&f, r#"{"mode":"direct"}"#).unwrap();
    assert_eq!(NetPolicy::load(&f).mode, Mode::Off, "unknown mode");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_page_lines_say_what_the_sandbox_can_reach() {
    assert_eq!(describe(&NetPolicy::default()), ["Off: no network"]);
    let d = describe(&outbound(&[]));
    assert!(d[1].contains("private networks and the computer itself are blocked"));
    let d = describe(&outbound(&["example.org:443", "203.0.113.0/24"]));
    assert_eq!(d[1], "Allowed only: example.org:443, 203.0.113.0/24");
    assert!(Mode::HostShared.label().contains("computer's own services"));
}
