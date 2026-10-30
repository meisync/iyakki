//! Tests for the route, DNS and kill-switch plans (plan section 2.3). Addresses are documentation ranges (RFC 5737, 3849).

use std::net::IpAddr;

use crate::net::{default_cluster_cidrs, Cidr, Context};
use crate::plan::*;
use crate::VpnError;

fn c(s: &str) -> Cidr {
    Cidr::parse(s).unwrap()
}
fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}
fn ctx() -> Context {
    Context { cluster: default_cluster_cidrs(), lan: vec![c("192.168.1.0/24")], tunnel: vec![c("10.8.0.2/24")] }
}
fn input<'a>(allowed: &'a [Cidr], endpoints: &'a [IpAddr], ctx: &'a Context) -> RouteInput<'a> {
    RouteInput { ifname: "wg0", physical_if: "eth0", endpoints, allowed_ips: allowed, gateway4: Some("192.168.1.1".parse().unwrap()), gateway6: None, ctx }
}
fn dests(r: &[Route]) -> Vec<String> {
    r.iter().map(|r| format!("{}{}{}", r.dest, r.via.map(|v| format!(" via {v}")).unwrap_or_default(), format_args!(" dev {}", r.dev))).collect()
}

#[test]
fn a_full_tunnel_pins_the_endpoint_and_uses_the_two_halves() {
    let x = ctx();
    let allowed = [c("0.0.0.0/0")];
    let endpoints = [ip("203.0.113.9")];
    let p = plan_routes(&input(&allowed, &endpoints, &x)).unwrap();
    assert_eq!(dests(&p.add), ["203.0.113.9/32 via 192.168.1.1 dev eth0", "0.0.0.0/1 dev wg0", "128.0.0.0/1 dev wg0"]);
    assert!(p.full_tunnel_v4 && !p.full_tunnel_v6);
    // undo in the reverse order; the default route was never touched, so nothing else is needed
    assert_eq!(dests(&p.remove), ["128.0.0.0/1 dev wg0", "0.0.0.0/1 dev wg0", "203.0.113.9/32 via 192.168.1.1 dev eth0"]);
    assert!(p.add.iter().all(|r| !r.dest.is_default()));
}

#[test]
fn ipv6_full_tunnel_and_dual_stack() {
    let x = ctx();
    let allowed = [c("0.0.0.0/0"), c("::/0")];
    let endpoints = [ip("203.0.113.9")];
    let mut i = input(&allowed, &endpoints, &x);
    i.gateway6 = Some("fe80::1".parse().unwrap());
    let p = plan_routes(&i).unwrap();
    assert!(p.full_tunnel_v4 && p.full_tunnel_v6);
    let d = dests(&p.add);
    assert!(d.contains(&"::/1 dev wg0".to_string()) && d.contains(&"8000::/1 dev wg0".to_string()));
    // an IPv6 endpoint uses the IPv6 gateway
    let e6 = [ip("2001:db8::9")];
    let p = plan_routes(&RouteInput { endpoints: &e6, ..i }).unwrap();
    assert_eq!(dests(&p.add)[0], "2001:db8::9/128 via fe80::1 dev eth0");
}

#[test]
fn split_tunnel_routes_only_what_is_listed_and_does_not_pin_a_connected_endpoint() {
    let x = ctx();
    let allowed = [c("10.8.0.9/24"), c("10.8.0.0/24"), c("203.0.113.0/24")];
    let endpoints = [ip("192.168.1.50")]; // on the connected LAN
    let p = plan_routes(&input(&allowed, &endpoints, &x)).unwrap();
    assert_eq!(dests(&p.add), ["10.8.0.0/24 dev wg0", "203.0.113.0/24 dev wg0"]); // masked and de-duplicated
    assert!(!p.full_tunnel_v4);
    assert!(p.warnings.is_empty());
}

#[test]
fn cluster_and_lan_stay_reachable_because_nothing_overlaps_them_but_halves() {
    let x = ctx();
    let allowed = [c("0.0.0.0/0")];
    let p = plan_routes(&input(&allowed, &[], &x)).unwrap();
    // every added route is a /1 half, so the /16 cluster routes and the /24 LAN route are all longer and win
    for r in &p.add {
        assert_eq!(r.dest.prefix, 1);
        for keep in x.cluster.iter().chain(&x.lan) {
            assert!(keep.prefix > r.dest.prefix && r.dest.contains(keep) || !r.dest.overlaps(keep));
        }
    }
}

#[test]
fn unsafe_allowed_ips_are_refused_against_the_real_network() {
    let x = ctx();
    let e = [ip("203.0.113.9")];
    for bad in ["10.42.0.0/24", "10.43.0.0/16", "127.0.0.0/8", "169.254.169.254/32", "192.168.1.0/24", "192.168.0.0/16", "10.0.0.0/8"] {
        let allowed = [c(bad)];
        assert!(matches!(plan_routes(&input(&allowed, &e, &x)), Err(VpnError::Unsafe(_))), "{bad}");
    }
    // a different LAN at connect time than at import changes the verdict
    let other = Context { lan: vec![c("172.16.5.0/24")], ..ctx() };
    let allowed = [c("172.16.5.0/24")];
    assert!(plan_routes(&input(&allowed, &e, &other)).is_err());
    assert!(plan_routes(&input(&allowed, &e, &ctx())).is_ok()); // only a warning there
    assert_eq!(plan_routes(&input(&allowed, &e, &ctx())).unwrap().warnings.len(), 1);
}

#[test]
fn no_gateway_gives_an_on_link_route_and_a_warning() {
    let x = ctx();
    let allowed = [c("0.0.0.0/0")];
    let e = [ip("203.0.113.9")];
    let mut i = input(&allowed, &e, &x);
    i.gateway4 = None;
    let p = plan_routes(&i).unwrap();
    assert_eq!(dests(&p.add)[0], "203.0.113.9/32 dev eth0");
    assert_eq!(p.warnings.len(), 1);
}

#[test]
fn interface_names_are_checked_and_commands_are_fixed_argument_lists() {
    let x = ctx();
    let allowed = [c("10.8.0.0/24")];
    for bad in ["", "wg0;reboot", "a b", "../x", "0123456789abcdef"] {
        let mut i = input(&allowed, &[], &x);
        i.ifname = bad;
        assert!(plan_routes(&i).is_err(), "{bad:?}");
        assert!(link_add_args(bad).is_err() && link_del_args(bad).is_err() && link_up_args(bad, None).is_err());
        assert!(addr_add_args(bad, &c("10.0.0.2/24")).is_err());
    }
    assert_eq!(link_add_args("wg0").unwrap(), ["link", "add", "dev", "wg0", "type", "wireguard"]);
    assert_eq!(link_up_args("wg0", Some(1380)).unwrap(), ["link", "set", "dev", "wg0", "mtu", "1380", "up"]);
    assert_eq!(link_up_args("wg0", None).unwrap(), ["link", "set", "dev", "wg0", "up"]);
    assert_eq!(addr_add_args("wg0", &c("10.8.0.2/24")).unwrap(), ["address", "add", "10.8.0.2/24", "dev", "wg0"]);
    assert_eq!(link_del_args("wg0").unwrap(), ["link", "del", "dev", "wg0"]);
    let r = Route { dest: c("203.0.113.9/32"), via: Some(ip("192.168.1.1")), dev: "eth0".into() };
    assert_eq!(r.ip_args(true), ["route", "add", "203.0.113.9/32", "via", "192.168.1.1", "dev", "eth0"]);
    assert_eq!(r.ip_args(false)[1], "del");
    let r = Route { dest: c("0.0.0.0/1"), via: None, dev: "wg0".into() };
    assert_eq!(r.ip_args(true), ["route", "add", "0.0.0.0/1", "dev", "wg0"]);
}

#[test]
fn dns_text_and_restore() {
    let t = resolv_conf(&[ip("10.8.0.1"), ip("2001:db8::53")]);
    assert_eq!(t, format!("{DNS_MARKER}\nnameserver 10.8.0.1\nnameserver 2001:db8::53\n"));
    let original = "nameserver 192.168.1.1\nsearch lan\n";
    // still ours: the original comes back
    assert_eq!(resolv_restore(&t, Some(original)).as_deref(), Some(original));
    // a DHCP renewal replaced it meanwhile: leave the new file alone
    assert_eq!(resolv_restore("nameserver 192.168.1.1\n", Some(original)), None);
    // no original was saved: nothing to write back
    assert_eq!(resolv_restore(&t, None), None);
    assert_eq!(resolv_restore("", Some(original)), None);
    // nothing but addresses ever reaches the file, one per line
    assert!(t.lines().skip(1).all(|l| l.strip_prefix("nameserver ").is_some_and(|a| a.parse::<IpAddr>().is_ok())));
    assert!(dns_leak_warning(true, &[]).is_some());
    assert!(dns_leak_warning(true, &[ip("10.8.0.1")]).is_none());
    assert!(dns_leak_warning(false, &[]).is_none());
}

fn base<'a>(eps: &'a [(IpAddr, u16, Proto)], lan: &'a [Cidr], cluster: &'a [Cidr], phys: &'a [String]) -> GuardBase<'a> {
    GuardBase { ifname: "wg0", physical_ifs: phys, endpoints: eps, lan, cluster, cluster_if_prefixes: &["cni", "flannel", "veth"], dev_port: DEV_PORT }
}
fn ks_input<'a>(eps: &'a [(IpAddr, u16, Proto)], lan: &'a [Cidr], cluster: &'a [Cidr], phys: &'a [String]) -> KillSwitchInput<'a> {
    KillSwitchInput { base: base(eps, lan, cluster, phys), block_dns_outside: false }
}
fn pkt(oif: &str, dst: &str, proto: Proto, sport: u16, dport: u16) -> Packet {
    Packet { oif: oif.into(), daddr: ip(dst), proto, sport, dport }
}

#[test]
fn the_kill_switch_lets_through_only_what_the_plan_names() {
    let eps = [(ip("203.0.113.9"), 51820u16, Proto::Udp)];
    let lan = [c("192.168.1.0/24")];
    let cl = default_cluster_cidrs();
    let phys = ["eth0".to_string()];
    let ks = kill_switch(&ks_input(&eps, &lan, &cl, &phys)).unwrap();
    let ok = Verdict::Accept;
    let no = Verdict::Drop;
    // loopback, the tunnel, the endpoint on the physical interface, DHCP
    assert_eq!(ks.evaluate(&pkt("lo", "127.0.0.1", Proto::Tcp, 1, 80)), ok);
    assert_eq!(ks.evaluate(&pkt("wg0", "8.8.8.8", Proto::Tcp, 1, 443)), ok);
    assert_eq!(ks.evaluate(&pkt("eth0", "203.0.113.9", Proto::Udp, 5555, 51820)), ok);
    assert_eq!(ks.evaluate(&pkt("eth0", "255.255.255.255", Proto::Udp, 68, 67)), ok);
    assert_eq!(ks.evaluate(&pkt("eth0", "ff02::1:2", Proto::Udp, 546, 547)), ok);
    assert_eq!(ks.evaluate(&pkt("eth0", "ff02::1", Proto::Icmpv6, 0, 0)), ok);
    // the cluster
    assert_eq!(ks.evaluate(&pkt("cni0", "10.42.1.5", Proto::Tcp, 1, 80)), ok);
    assert_eq!(ks.evaluate(&pkt("flannel.1", "10.42.2.5", Proto::Tcp, 1, 80)), ok);
    assert_eq!(ks.evaluate(&pkt("eth0", "10.43.0.10", Proto::Udp, 1, 53)), ok);
    // the local network and developer-mode replies are ALWAYS let through (owner requirement 2026-10-28)
    assert_eq!(ks.evaluate(&pkt("eth0", "192.168.1.20", Proto::Tcp, 1, 22)), ok);
    assert_eq!(ks.evaluate(&pkt("eth0", "192.168.1.20", Proto::Tcp, DEV_PORT, 51234)), ok);
    assert_eq!(ks.evaluate(&pkt("eth0", "10.99.0.4", Proto::Tcp, DEV_PORT, 51234)), ok); // a paired computer on another private net
    // everything else leaves nothing: the internet, the wrong port or protocol to the server, another interface to it
    assert_eq!(ks.evaluate(&pkt("eth0", "8.8.8.8", Proto::Tcp, 1, 443)), no);
    assert_eq!(ks.evaluate(&pkt("eth0", "8.8.8.8", Proto::Tcp, 1, DEV_PORT)), no); // the port is the source port of replies, not a way out
    assert_eq!(ks.evaluate(&pkt("eth0", "203.0.113.9", Proto::Tcp, 1, 51820)), no);
    assert_eq!(ks.evaluate(&pkt("eth0", "203.0.113.9", Proto::Udp, 1, 443)), no);
    assert_eq!(ks.evaluate(&pkt("wlan0", "203.0.113.9", Proto::Udp, 1, 51820)), no);
    assert_eq!(ks.evaluate(&pkt("eth0", "2001:db8::1", Proto::Tcp, 1, 443)), no);
    assert_eq!(ks.rules.last().unwrap().verdict, Verdict::Drop);
}

#[test]
fn dns_blocking_and_roaming_interfaces() {
    let eps = [(ip("203.0.113.9"), 51820u16, Proto::Udp)];
    let lan = [c("192.168.1.0/24")];
    let cl = default_cluster_cidrs();
    let phys = ["eth0".to_string(), "wlan0".to_string()];
    let mut i = ks_input(&eps, &lan, &cl, &phys);
    let ks = kill_switch(&i).unwrap();
    assert_eq!(ks.evaluate(&pkt("wlan0", "203.0.113.9", Proto::Udp, 1, 51820)), Verdict::Accept); // after roaming
    assert_eq!(ks.evaluate(&pkt("eth0", "192.168.1.1", Proto::Udp, 1, 53)), Verdict::Accept);
    i.block_dns_outside = true;
    let ks = kill_switch(&i).unwrap();
    assert_eq!(ks.evaluate(&pkt("eth0", "192.168.1.1", Proto::Udp, 1, 53)), Verdict::Drop);
    assert_eq!(ks.evaluate(&pkt("eth0", "192.168.1.1", Proto::Tcp, 1, 53)), Verdict::Drop);
    assert_eq!(ks.evaluate(&pkt("wg0", "10.8.0.1", Proto::Udp, 1, 53)), Verdict::Accept);
    assert_eq!(ks.evaluate(&pkt("eth0", "10.43.0.10", Proto::Udp, 1, 53)), Verdict::Accept);
    assert_eq!(ks.evaluate(&pkt("eth0", "192.168.1.20", Proto::Tcp, 1, 22)), Verdict::Accept);
}

#[test]
fn the_kill_switch_table_has_the_documented_identity_and_renders_as_nft_text() {
    let eps = [(ip("203.0.113.9"), 51820u16, Proto::Udp)];
    let cl = default_cluster_cidrs();
    let phys = ["eth0".to_string()];
    let lan = [c("192.168.1.0/24")];
    let ks = kill_switch(&ks_input(&eps, &lan, &cl, &phys)).unwrap();
    let t = ks.render();
    assert!(t.starts_with("table inet iyakki_vpn {\n  chain output {\n    type filter hook output priority -10; policy accept;\n"));
    assert!(t.contains("oifname \"lo\" accept"));
    assert!(t.contains("oifname \"eth0\" ip daddr 203.0.113.9/32 udp dport 51820 accept"));
    assert!(t.contains("udp sport 68 udp dport 67 accept"));
    assert!(t.contains("tcp sport 7979 accept comment \"developer mode replies\""));
    assert!(t.contains("oifname \"cni*\" accept"));
    assert!(t.contains("ip daddr 10.43.0.0/16 accept"));
    assert!(t.contains("ip daddr 192.168.1.0/24 accept comment \"the local network\""));
    assert!(t.contains("ip6 daddr fe80::/10 meta l4proto icmpv6 accept"));
    assert!(t.ends_with("  }\n}\n"));
    assert!(t.contains("drop comment \"everything else\""));
    assert_eq!((KS_TABLE, KS_FAMILY, KS_CHAIN, KS_PRIORITY), ("iyakki_vpn", "inet", "output", -10));
}

#[test]
fn bad_names_never_reach_the_guards() {
    let cl = default_cluster_cidrs();
    let phys = ["eth0".to_string()];
    let mut i = ks_input(&[], &[], &cl, &phys);
    i.base.ifname = "wg0\"; drop";
    assert!(kill_switch(&i).is_err());
    let bad_phys = ["eth0\" accept #".to_string()];
    let eps = [(ip("203.0.113.9"), 1, Proto::Udp)];
    assert!(kill_switch(&ks_input(&eps, &[], &cl, &bad_phys)).is_err());
    let mut j = ks_input(&[], &[], &cl, &phys);
    j.base.cluster_if_prefixes = &["cni\" accept"];
    assert!(kill_switch(&j).is_err());
    j.base.cluster_if_prefixes = &[""];
    assert!(kill_switch(&j).is_err());
    let nets: [(Cidr, bool); 0] = [];
    let mut f = FailClosedInput { base: base(&[], &[], &cl, &phys), nets: &nets };
    f.base.ifname = "wg 0";
    assert!(fail_closed_guard(&f).is_err());
}

// ---- HYBRID ------------------------------------------------------------------------------------------------------------

fn nets(v: &[(&str, bool)]) -> Vec<(Cidr, bool)> {
    v.iter().map(|(s, i)| (c(s), *i)).collect()
}

#[test]
fn hybrid_routes_are_only_the_included_ranges_and_never_the_halves_or_the_default() {
    let x = ctx();
    let n = nets(&[("203.0.113.0/24", true), ("198.51.100.0/24", true), ("203.0.113.7/32", false)]);
    let allowed = [c("0.0.0.0/0")];
    let e = [ip("203.0.113.50")]; // the VPN server sits inside an included range
    let p = plan_hybrid_routes(&HybridRouteInput { ifname: "wg0", physical_if: "eth0", endpoints: &e, allowed_ips: &allowed, gateway4: Some("192.168.1.1".parse().unwrap()), gateway6: None, ctx: &x, nets: &n }).unwrap();
    assert_eq!(
        dests(&p.add),
        ["203.0.113.50/32 via 192.168.1.1 dev eth0", "203.0.113.0/24 dev wg0", "198.51.100.0/24 dev wg0", "203.0.113.7/32 via 192.168.1.1 dev eth0"]
    );
    assert!(p.add.iter().all(|r| r.dest.prefix > 1), "no default route and no halves");
    assert!(!p.full_tunnel_v4 && !p.full_tunnel_v6);
    assert_eq!(p.remove.len(), p.add.len());
    assert_eq!(p.remove[0], *p.add.last().unwrap());
}

#[test]
fn hybrid_routes_outside_allowed_ips_are_warned_not_added_and_unsafe_ones_are_refused() {
    let x = ctx();
    let allowed = [c("198.51.100.0/24")];
    let n = nets(&[("203.0.113.0/24", true), ("198.51.100.0/25", true)]);
    let inp = |n: &'_ [(Cidr, bool)]| HybridRouteInput { ifname: "wg0", physical_if: "eth0", endpoints: &[], allowed_ips: Box::leak(Box::new(allowed)), gateway4: None, gateway6: None, ctx: Box::leak(Box::new(x.clone())), nets: Box::leak(n.to_vec().into_boxed_slice()) };
    let p = plan_hybrid_routes(&inp(&n)).unwrap();
    assert_eq!(dests(&p.add), ["198.51.100.0/25 dev wg0"]);
    assert_eq!(p.warnings.len(), 1);
    for bad in ["10.42.0.0/24", "10.43.0.0/16", "127.0.0.0/8", "169.254.169.254/32", "192.168.1.0/24", "0.0.0.0/0", "0.0.0.0/1", "128.0.0.0/1"] {
        let n = nets(&[(bad, true)]);
        assert!(plan_hybrid_routes(&inp(&n)).is_err(), "{bad}");
    }
    // exclusions are never an error
    assert!(plan_hybrid_routes(&inp(&nets(&[("10.42.0.0/24", false)]))).unwrap().add.is_empty());
    let mut i = inp(&n);
    i.ifname = "wg0;x";
    assert!(plan_hybrid_routes(&i).is_err());
}

fn fc_spec(n: &[(Cidr, bool)]) -> KillSwitchSpec {
    let cl = default_cluster_cidrs();
    let phys: &'static [String] = Box::leak(vec!["eth0".to_string()].into_boxed_slice());
    let lan: &'static [Cidr] = Box::leak(vec![c("192.168.1.0/24")].into_boxed_slice());
    let cl: &'static [Cidr] = Box::leak(cl.into_boxed_slice());
    let eps: &'static [(IpAddr, u16, Proto)] = Box::leak(vec![(ip("203.0.113.9"), 51820u16, Proto::Udp)].into_boxed_slice());
    let nets: &'static [(Cidr, bool)] = Box::leak(n.to_vec().into_boxed_slice());
    fail_closed_guard(&FailClosedInput { base: base(eps, lan, cl, phys), nets }).unwrap()
}

#[test]
fn fail_closed_drops_matched_destinations_that_would_leave_direct_and_nothing_else() {
    let spec = fc_spec(&nets(&[("198.51.100.0/24", true), ("198.51.100.9/32", false)]));
    let spec = spec.with_members(SET_V4, vec![c("192.0.2.77/32")]).with_members(SET_V6, vec![c("2001:db8:77::1/128")]);
    let (ok, no) = (Verdict::Accept, Verdict::Drop);
    // the tunnel is down: the matched packet would go out of eth0
    assert_eq!(spec.evaluate(&pkt("eth0", "198.51.100.20", Proto::Tcp, 1, 443)), no);
    assert_eq!(spec.evaluate(&pkt("eth0", "192.0.2.77", Proto::Tcp, 1, 443)), no);
    assert_eq!(spec.evaluate(&pkt("wlan0", "2001:db8:77::1", Proto::Tcp, 1, 443)), no);
    // the same packets through the tunnel pass
    assert_eq!(spec.evaluate(&pkt("wg0", "198.51.100.20", Proto::Tcp, 1, 443)), ok);
    assert_eq!(spec.evaluate(&pkt("wg0", "192.0.2.77", Proto::Tcp, 1, 443)), ok);
    // an exclusion inside the included range, and everything unmatched, goes direct as before
    assert_eq!(spec.evaluate(&pkt("eth0", "198.51.100.9", Proto::Tcp, 1, 443)), ok);
    assert_eq!(spec.evaluate(&pkt("eth0", "8.8.8.8", Proto::Tcp, 1, 443)), ok);
    assert_eq!(spec.evaluate(&pkt("eth0", "192.0.2.78", Proto::Tcp, 1, 443)), ok);
    // always allowed: loopback, LAN, cluster, DHCP, developer-mode replies, the VPN server itself
    assert_eq!(spec.evaluate(&pkt("lo", "127.0.0.1", Proto::Tcp, 1, 80)), ok);
    assert_eq!(spec.evaluate(&pkt("eth0", "192.168.1.20", Proto::Tcp, 1, 22)), ok);
    assert_eq!(spec.evaluate(&pkt("cni0", "10.42.0.9", Proto::Tcp, 1, 80)), ok);
    assert_eq!(spec.evaluate(&pkt("eth0", "10.43.0.1", Proto::Tcp, 1, 443)), ok);
    assert_eq!(spec.evaluate(&pkt("eth0", "255.255.255.255", Proto::Udp, 68, 67)), ok);
    assert_eq!(spec.evaluate(&pkt("eth0", "192.0.2.77", Proto::Tcp, DEV_PORT, 40000)), ok); // a reply from the developer port
    assert_eq!(spec.evaluate(&pkt("eth0", "203.0.113.9", Proto::Udp, 1, 51820)), ok);
    // there is no catch-all drop in the hybrid guard
    assert!(spec.rules.iter().all(|r| r.m != Match::default()));
}

#[test]
fn the_fail_closed_table_renders_with_its_sets() {
    let t = fc_spec(&nets(&[("198.51.100.0/24", true)])).render();
    assert!(t.contains("set vpn_dest4 { type ipv4_addr; flags interval, timeout; }"));
    assert!(t.contains("set vpn_dest6 { type ipv6_addr; flags interval, timeout; }"));
    assert!(t.contains("oifname != \"wg0\" ip daddr 198.51.100.0/24 drop"));
    assert!(t.contains("oifname != \"wg0\" ip daddr @vpn_dest4 drop"));
    assert!(t.contains("oifname != \"wg0\" ip6 daddr @vpn_dest6 drop"));
    assert!(t.contains("tcp sport 7979 accept"));
    assert!(!t.contains("everything else"));
}

#[test]
fn dns_capture_redirects_dns_without_touching_resolv_conf_and_spares_the_forwarder() {
    let cap = dns_capture(5354).unwrap();
    assert!(cap.captures(Proto::Udp, 53, 0));
    assert!(cap.captures(Proto::Tcp, 53, 0));
    assert!(!cap.captures(Proto::Udp, 53, DNS_MARK)); // the forwarder's own upstream queries
    assert!(!cap.captures(Proto::Udp, 443, 0));
    assert!(!cap.captures(Proto::Icmpv6, 53, 0));
    for bad in [0u16, 53, 80, 1023] {
        assert!(dns_capture(bad).is_err(), "{bad}");
    }
    let t = cap.render();
    assert!(t.starts_with("table inet iyakki_vpn_dns {"));
    assert!(t.contains("type nat hook output priority -100"));
    assert!(t.contains("meta mark 0x49560001 return"));
    assert!(t.contains("udp dport 53 redirect to :5354") && t.contains("tcp dport 53 redirect to :5354"));
    assert!(!t.contains("resolv"));
}
