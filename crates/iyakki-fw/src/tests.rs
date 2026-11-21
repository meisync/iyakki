//! Tests with fakes only: nothing here touches the machine's real firewall (the kernel test is in `tests/kernel.rs` and
//! runs in a private network namespace, ignored by default).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use crate::backend::{BackendError, FakeBackend};
use crate::blocked::{self, Blocked, BlockLog};
use crate::engine::{Clock, Engine, Resolver, RuleState, Saved, Status, Store};
use crate::model::{Config, Kind, Ping, Preset, Proto, Resolved, Rule};
use crate::ruleset::{build, eval, Ctx, Packet};
use crate::validate::{check_rule, parse_ports};

// ------------------------------------------------------------------ fakes

#[derive(Clone)]
struct FakeClock(Arc<AtomicI64>);
impl Clock for FakeClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Default)]
struct FakeResolver(Arc<Mutex<HashMap<String, Result<Vec<IpAddr>, String>>>>);
impl FakeResolver {
    fn set(&self, n: &str, a: &[&str]) {
        self.0.lock().unwrap().insert(n.into(), Ok(a.iter().map(|x| x.parse().unwrap()).collect()));
    }
    fn fail(&self, n: &str) {
        self.0.lock().unwrap().insert(n.into(), Err("no such host".into()));
    }
}
impl Resolver for FakeResolver {
    fn resolve(&self, name: &str) -> Result<Vec<IpAddr>, String> {
        self.0.lock().unwrap().get(name).cloned().unwrap_or_else(|| Err("no such host".into()))
    }
}

#[derive(Clone, Default)]
struct MemStore {
    saved: Arc<Mutex<Option<Saved>>>,
    fail_load: Arc<Mutex<bool>>,
    saves: Arc<Mutex<u32>>,
}
impl Store for MemStore {
    fn load(&self) -> Result<Option<Saved>, String> {
        if *self.fail_load.lock().unwrap() {
            return Err("unreadable".into());
        }
        Ok(self.saved.lock().unwrap().clone())
    }
    fn save(&self, s: &Saved) -> Result<(), String> {
        *self.saved.lock().unwrap() = Some(s.clone());
        *self.saves.lock().unwrap() += 1;
        Ok(())
    }
}

struct Rig {
    e: Engine,
    be: FakeBackend,
    clock: FakeClock,
    dns: FakeResolver,
    store: MemStore,
    audit: Arc<Mutex<Vec<(String, String)>>>,
}

fn rig() -> Rig {
    rig_with(MemStore::default())
}

fn rig_with(store: MemStore) -> Rig {
    let be = FakeBackend::default();
    let clock = FakeClock(Arc::new(AtomicI64::new(1_000_000)));
    let dns = FakeResolver::default();
    let mut e = Engine::new(Box::new(be.clone()), Box::new(clock.clone()), Box::new(dns.clone()), Box::new(store.clone()));
    let audit: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let a2 = audit.clone();
    e.set_audit(Arc::new(move |ev, d| a2.lock().unwrap().push((ev.into(), d.into()))));
    Rig { e, be, clock, dns, store, audit }
}

impl Rig {
    fn advance(&self, s: i64) {
        self.clock.0.fetch_add(s, Ordering::SeqCst);
    }
    fn events(&self) -> Vec<String> {
        self.audit.lock().unwrap().iter().map(|(e, _)| e.clone()).collect()
    }
}

fn rule(kind: Kind, v: &str) -> Rule {
    Rule::new(kind, v)
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn golden(name: &str, actual: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.txt"));
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing golden file {}", path.display()));
    assert_eq!(want, actual, "golden file {name} differs (run with UPDATE_GOLDEN=1 to rewrite after checking the change)");
}

fn text(cfg: &Config, resolved: &Resolved, ctx: &Ctx) -> String {
    build(cfg, resolved, ctx).unwrap().ruleset.to_text()
}

fn only_user_rules(t: &str) -> String {
    // the fixed base and the final counters are checked by their own tests; the scenarios show what the rules add
    t.lines().filter(|l| l.contains("# rule ") || l.contains("# preset") || l.contains("# remote diagnostics")).collect::<Vec<_>>().join("\n") + "\n"
}

// ------------------------------------------------------------------ the generator

#[test]
fn golden_default_policy_blocks_incoming_and_keeps_the_basics() {
    golden("default", &text(&Config::default(), &Resolved::default(), &Ctx::default()));
}

#[test]
fn golden_without_ipv6_and_without_container_trust_and_ping_modes() {
    let mut c = Config::default();
    c.trust_containers = false;
    c.presets.clear();
    c.ping = Ping::Off;
    golden("minimal_v4_only", &text(&c, &Resolved::default(), &Ctx { ipv6: false, ..Ctx::default() }));
    c.ping = Ping::Anyone;
    golden("ping_anyone", &text(&c, &Resolved::default(), &Ctx::default()));
}

#[test]
fn golden_each_rule_type() {
    let mut c = Config::default();
    c.presets.clear();
    c.ping = Ping::Off;
    c.add(rule(Kind::Ip, "198.51.100.7"));
    c.add(rule(Kind::Ip, "2001:db8::7"));
    c.add(rule(Kind::Range, "198.51.100.10-198.51.100.20"));
    c.add(rule(Kind::Range, "2001:db8::10 - 2001:db8::20"));
    c.add(rule(Kind::Cidr, "203.0.113.0/24"));
    c.add(rule(Kind::Cidr, "2001:db8:1::/48"));
    c.add(rule(Kind::Cidr, "203.0.113.5/32"));
    c.add(rule(Kind::Cidr, "10.0.0.0/13"));
    golden("rule_types", &only_user_rules(&text(&c, &Resolved::default(), &Ctx::default())));
}

#[test]
fn golden_ports_protocols_interface_and_comment() {
    let mut c = Config::default();
    c.presets.clear();
    let mut a = rule(Kind::Ip, "198.51.100.7");
    a.proto = Proto::Tcp;
    a.ports = "22".into();
    a.iface = "eth0".into();
    a.comment = "admin laptop".into();
    c.add(a);
    let mut b = rule(Kind::Cidr, "203.0.113.0/24");
    b.proto = Proto::Both;
    b.ports = "53, 8000-8100".into();
    c.add(b);
    let mut u = rule(Kind::Ip, "198.51.100.8");
    u.proto = Proto::Udp;
    u.ports = "5353".into();
    c.add(u);
    let mut n = rule(Kind::Ip, "198.51.100.9");
    n.proto = Proto::Tcp;
    c.add(n);
    golden("ports_proto_iface", &only_user_rules(&text(&c, &Resolved::default(), &Ctx::default())));
}

#[test]
fn golden_hostnames_with_a_fake_resolver() {
    let mut c = Config::default();
    c.presets.clear();
    let mut h = rule(Kind::Host, "Laptop.Example.com.");
    h.proto = Proto::Tcp;
    h.ports = "22".into();
    c.add(h);
    c.add(rule(Kind::Host, "nas.local"));
    let mut r = Resolved::default();
    r.addrs.insert("laptop.example.com".into(), vec![ip("198.51.100.4"), ip("2001:db8::4")]);
    // nas.local has no answer yet: it allows nothing
    let b = build(&c, &r, &Ctx::default()).unwrap();
    assert_eq!(b.hosts.len(), 2);
    assert!(b.hosts[1].addrs.is_empty());
    golden("hostnames", &only_user_rules(&b.ruleset.to_text()));
}

#[test]
fn golden_presets_and_the_ssh_boot_entry() {
    let mut c = Config::default();
    c.presets = Preset::ALL.to_vec();
    golden("presets_all", &only_user_rules(&text(&c, &Resolved::default(), &Ctx::default())));
    c.presets.clear();
    golden("ssh_boot", &only_user_rules(&text(&c, &Resolved::default(), &Ctx { ssh_boot: true, ..Ctx::default() })));
    // with the SSH preset on as well, port 22 is not written twice
    c.set_preset(Preset::Ssh, true);
    let t = text(&c, &Resolved::default(), &Ctx { ssh_boot: true, ..Ctx::default() });
    assert_eq!(t.matches("th dport 22 counter accept").count(), 6, "{t}");
    // a cloud machine's web mode is reachable from anywhere; a chosen port is allowed as well
    let mut c = Config::default();
    c.presets = vec![Preset::RemoteDesktop];
    golden("remote_cloud", &only_user_rules(&text(&c, &Resolved::default(), &Ctx { cloud: true, remote_port: Some(9443), ..Ctx::default() })));
}

#[test]
fn the_networks_of_a_vpn_count_as_local_only_when_asked_and_only_if_private() {
    use crate::ruleset::{vpn_network, Addr};
    let mut c = Config::default();
    c.presets = vec![Preset::Ssh];
    let nets = vec![vpn_network("100.64.1.2/24").unwrap()];
    assert_eq!(nets[0], Addr::Cidr("100.64.1.0".parse().unwrap(), 24), "host bits are cleared");
    let ctx = Ctx { vpn_nets: nets, ..Ctx::default() };
    let peer: IpAddr = "100.64.1.5".parse().unwrap();
    let ssh = |c: &Config, from: IpAddr| eval(&build(c, &Resolved::default(), &ctx).unwrap().ruleset, &Packet::new_tcp("wg0", from, 22));
    assert!(!ssh(&c, peer), "off by default: a VPN peer is not trusted for being on the VPN");
    c.vpn_local = true;
    assert!(ssh(&c, peer), "on: the tunnel's network is the local network");
    assert!(!ssh(&c, "100.64.2.5".parse().unwrap()), "only that network");
    assert!(ssh(&c, "10.8.0.5".parse().unwrap()), "the ordinary private ranges stay local as before");
    assert!(!ssh(&c, "203.0.113.9".parse().unwrap()));
    // ping follows the same rule
    c.ping = Ping::Local;
    let ping = |c: &Config| eval(&build(c, &Resolved::default(), &ctx).unwrap().ruleset, &Packet { iif: "wg0".into(), ct: crate::ruleset::CT_NEW, src: peer, l4: 1, dport: 0, icmp_type: 8 });
    assert!(ping(&c));
    c.vpn_local = false;
    assert!(!ping(&c));
    // what may not count as local
    for bad in ["8.8.8.0/24", "10.0.0.0/7", "0.0.0.0/0", "100.63.0.0/16", "2001:db8::/32", "10.0.0.1", "10.0.0.0/x", "fe80::/10"] {
        assert!(vpn_network(bad).is_err(), "{bad}");
    }
    assert!(vpn_network("100.64.1.1/10").is_ok() && vpn_network("fd12:3456::1/64").is_ok() && vpn_network("192.168.7.9/32").is_ok());
}

#[test]
fn turning_on_vpn_is_local_only_opens_and_turning_it_off_tightens() {
    let old = Config::default();
    let mut new = old.clone();
    new.vpn_local = true;
    assert!(new.only_opens_from(&old), "no lock-out from letting more in");
    assert!(!old.only_opens_from(&new), "taking it away asks to be kept");
    // an old export without the field still reads, with the setting off
    let mut j: serde_json::Value = serde_json::from_str(&old.to_json()).unwrap();
    j.as_object_mut().unwrap().remove("vpn_local");
    assert!(!Config::from_json(&j.to_string()).unwrap().vpn_local);
}

#[test]
fn expiry_ends_a_rule_and_the_disabled_switch_skips_it() {
    let mut c = Config::default();
    c.presets.clear();
    let mut a = rule(Kind::Ip, "198.51.100.7");
    a.expires = 100;
    let id = c.add(a);
    let mut off = rule(Kind::Ip, "198.51.100.8");
    off.enabled = false;
    c.add(off);
    let live = build(&c, &Resolved::default(), &Ctx { now: 99, ..Ctx::default() }).unwrap();
    assert!(live.ruleset.to_text().contains("198.51.100.7"));
    assert!(!live.ruleset.to_text().contains("198.51.100.8"), "disabled rules are not written");
    let gone = build(&c, &Resolved::default(), &Ctx { now: 100, ..Ctx::default() }).unwrap();
    assert!(!gone.ruleset.to_text().contains("198.51.100.7"));
    assert_eq!(gone.expired, vec![id]);
}

#[test]
fn too_many_combinations_is_refused_with_advice() {
    let mut c = Config::default();
    for i in 0..150 {
        let mut r = rule(Kind::Cidr, &format!("10.{i}.0.0/16"));
        r.ports = "1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16".into();
        r.proto = Proto::Both;
        c.add(r);
    }
    let e = build(&c, &Resolved::default(), &Ctx::default()).unwrap_err();
    assert!(e.contains("limit"), "{e}");
}

// ------------------------------------------------------------------ what the rules do to packets

fn default_rs() -> crate::ruleset::Ruleset {
    build(&Config::default(), &Resolved::default(), &Ctx::default()).unwrap().ruleset
}

#[test]
fn by_default_new_incoming_connections_are_blocked_and_the_basics_work() {
    let rs = default_rs();
    let public = ip("198.51.100.7");
    let lan = ip("192.168.1.20");
    assert!(!eval(&rs, &Packet::new_tcp("eth0", public, 22)), "ssh from the internet");
    assert!(!eval(&rs, &Packet::new_tcp("eth0", lan, 22)), "ssh is off until allowed");
    assert!(!eval(&rs, &Packet::new_tcp("eth0", lan, 8081)), "a share is off until allowed");
    assert!(eval(&rs, &Packet::new_tcp("eth0", lan, 8443)), "web mode from the local network is on by default");
    assert!(!eval(&rs, &Packet::new_tcp("eth0", public, 8443)), "but not from the internet");
    let mut est = Packet::new_tcp("eth0", public, 40000);
    est.ct = crate::ruleset::CT_ESTABLISHED;
    assert!(eval(&rs, &est), "replies to our own connections");
    est.ct = crate::ruleset::CT_INVALID;
    assert!(!eval(&rs, &est));
    assert!(eval(&rs, &Packet::new_tcp("lo", ip("127.0.0.1"), 5000)), "loopback");
    for i in ["cni0", "flannel.1", "veth1234", "docker0", "podman1"] {
        assert!(eval(&rs, &Packet::new_tcp(i, ip("10.42.0.5"), 6443)), "{i}: pods reach the host");
    }
    let mut dhcp = Packet::new_tcp("eth0", public, 68);
    dhcp.l4 = 17;
    assert!(eval(&rs, &dhcp), "DHCP replies");
    let mut ping = Packet::new_tcp("eth0", lan, 0);
    ping.l4 = 1;
    ping.icmp_type = 8;
    assert!(eval(&rs, &ping), "ping from the local network");
    ping.src = public;
    assert!(!eval(&rs, &ping), "not from the internet");
    let mut nd = Packet::new_tcp("eth0", ip("fe80::1"), 0);
    nd.l4 = 58;
    nd.icmp_type = 135;
    assert!(eval(&rs, &nd), "IPv6 neighbour discovery");
    nd.icmp_type = 128;
    assert!(eval(&rs, &nd), "ping from a link-local address");
    let mut d6 = Packet::new_tcp("eth0", ip("fe80::1"), 546);
    d6.l4 = 17;
    assert!(eval(&rs, &d6), "DHCPv6 reply");
}

#[test]
fn allowed_sources_ports_and_protocols_get_in_and_nothing_else_does() {
    let mut c = Config::default();
    c.presets.clear();
    let mut a = rule(Kind::Range, "198.51.100.10-198.51.100.20");
    a.proto = Proto::Tcp;
    a.ports = "8000-8100,22".into();
    c.add(a);
    let mut b = rule(Kind::Cidr, "2001:db8:1::/48");
    b.proto = Proto::Udp;
    b.ports = "53".into();
    c.add(b);
    let rs = build(&c, &Resolved::default(), &Ctx::default()).unwrap().ruleset;
    assert!(eval(&rs, &Packet::new_tcp("eth0", ip("198.51.100.10"), 8000)));
    assert!(eval(&rs, &Packet::new_tcp("eth0", ip("198.51.100.20"), 22)));
    assert!(!eval(&rs, &Packet::new_tcp("eth0", ip("198.51.100.21"), 22)), "one past the end of the range");
    assert!(!eval(&rs, &Packet::new_tcp("eth0", ip("198.51.100.9"), 22)), "one before");
    assert!(!eval(&rs, &Packet::new_tcp("eth0", ip("198.51.100.15"), 8101)), "port outside");
    let mut udp = Packet::new_tcp("eth0", ip("198.51.100.15"), 22);
    udp.l4 = 17;
    assert!(!eval(&rs, &udp), "tcp rule does not allow udp");
    let mut v6 = Packet::new_tcp("eth0", ip("2001:db8:1:ffff::1"), 53);
    v6.l4 = 17;
    assert!(eval(&rs, &v6));
    v6.src = ip("2001:db8:2::1");
    assert!(!eval(&rs, &v6));
    v6.l4 = 6;
    v6.src = ip("2001:db8:1::1");
    assert!(!eval(&rs, &v6), "udp rule does not allow tcp");
}

#[test]
fn only_our_own_table_is_ever_touched_and_k3s_networks_are_not_cut_off() {
    // the backend has two operations, both about our table; the ruleset never mentions flush or other tables
    let t = default_rs().to_text();
    assert!(t.starts_with("table inet iyakki_fw {"));
    assert!(!t.contains("flush") && !t.contains("ruleset"));
    let mut r = rig();
    r.e.propose(Config::default(), "admin").unwrap();
    assert!(r.be.calls().iter().all(|c| c.starts_with("replace") || c == "remove"), "{:?}", r.be.calls());
}

// ------------------------------------------------------------------ validation

#[test]
fn addresses_ranges_cidrs_and_ports_are_checked() {
    let bad_ip = ["", "999.1.1.1", "1.2.3", "1.2.3.4/24", "::g", "0.0.0.0", "::", "224.0.0.1", "255.255.255.255", "fe80::1%eth0", "1.2.3.4; drop", "1.2.3.4\n5"];
    for v in bad_ip {
        assert!(check_rule(&rule(Kind::Ip, v)).is_err(), "{v:?} should be refused");
    }
    assert!(check_rule(&rule(Kind::Ip, "  192.168.1.5 ")).is_ok());
    assert!(check_rule(&rule(Kind::Ip, "2001:db8::1")).is_ok());
    let e = check_rule(&rule(Kind::Range, "10.0.0.9-10.0.0.1")).unwrap_err();
    assert!(e.contains("reversed"), "{e}");
    assert!(check_rule(&rule(Kind::Range, "10.0.0.1-::2")).unwrap_err().contains("same kind"));
    assert!(check_rule(&rule(Kind::Range, "10.0.0.1")).is_err());
    let huge = check_rule(&rule(Kind::Range, "1.0.0.0-200.0.0.0")).unwrap();
    assert!(huge.warnings.iter().any(|w| w.contains("very large")), "{:?}", huge.warnings);
    let one = check_rule(&rule(Kind::Range, "10.0.0.1-10.0.0.1")).unwrap();
    assert!(matches!(one.src, crate::validate::Src::Ip(_)), "a one-address range is an address");
    let all = check_rule(&rule(Kind::Cidr, "0.0.0.0/0")).unwrap();
    assert!(all.warnings.iter().any(|w| w.contains("allows everyone")), "{:?}", all.warnings);
    assert!(check_rule(&rule(Kind::Cidr, "::/0")).unwrap().warnings.iter().any(|w| w.contains("everyone")));
    assert!(check_rule(&rule(Kind::Cidr, "10.0.0.5/24")).unwrap_err().contains("10.0.0.0"), "host bits are explained");
    assert!(check_rule(&rule(Kind::Cidr, "10.0.0.0/33")).is_err());
    assert!(check_rule(&rule(Kind::Cidr, "::/129")).is_err());
    assert!(check_rule(&rule(Kind::Cidr, "10.0.0.0/x")).is_err());
    assert!(check_rule(&rule(Kind::Cidr, "10.0.0.0")).is_err());
    // KI-1016: private and link-local ranges never get the "big part of the internet" warning; a public /8 still does.
    for p in ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "fc00::/7", "fe80::/10"] {
        let k = check_rule(&rule(Kind::Cidr, p)).unwrap();
        assert!(k.warnings.iter().all(|w| !w.contains("very large")), "{p}: {:?}", k.warnings);
    }
    let public8 = check_rule(&rule(Kind::Cidr, "8.0.0.0/8")).unwrap();
    assert!(public8.warnings.iter().any(|w| w.contains("/8 is a very large")), "{:?}", public8.warnings);
    let ula = check_rule(&rule(Kind::Range, "fc00::-fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff")).unwrap();
    assert!(ula.warnings.iter().all(|w| !w.contains("very large")), "{:?}", ula.warnings);
    assert!(parse_ports("0").is_err() && parse_ports("65536").is_err() && parse_ports("-1").is_err());
    assert!(parse_ports("1,65535,80-443").is_ok());
    assert!(parse_ports("90-80").unwrap_err().contains("reversed"));
    assert!(parse_ports("a").is_err() && parse_ports("80,,81").is_err() && parse_ports("80;81").is_err());
    assert!(parse_ports(&(1..=17).map(|i| i.to_string()).collect::<Vec<_>>().join(",")).is_err(), "too many items");
    assert_eq!(parse_ports("").unwrap(), vec![]);
}

#[test]
fn host_names_follow_the_dns_syntax_and_nothing_else_gets_through() {
    for ok in ["laptop", "a.b.example.com", "nas-1.local", "xn--bcher-kva.example", "Example.COM.", "a1.b2"] {
        assert!(check_rule(&rule(Kind::Host, ok)).is_ok(), "{ok}");
    }
    let bad = [
        "", ".", "..", "a..b", ".a.com", "-a.com", "a-.com", "a_b.com", "_dmarc.example.com", "a b.com", "a;b.com", "a\nb.com", "a\"b.com", "a'b.com", "a$(x).com", "a&b", "a/b", "a,b", "a#b.com",
        "bücher.example", "例え.jp", "*.example.com", "*", "a.b.123", "1.2.3.4", "::1", "a..", &"a".repeat(64), &format!("{}.com", "a".repeat(64)), &["abcdefghi"; 30].join("."),
    ];
    for v in bad {
        assert!(check_rule(&rule(Kind::Host, v)).is_err(), "{v:?} should be refused");
    }
    assert!(check_rule(&rule(Kind::Host, "*.example.com")).unwrap_err().contains("Wildcard"));
    assert!(check_rule(&rule(Kind::Host, "bücher.example")).unwrap_err().contains("xn--"));
    assert!(check_rule(&rule(Kind::Host, "a_b.com")).unwrap_err().contains("Underscore"));
    let w = check_rule(&rule(Kind::Host, "a.example.com")).unwrap().warnings;
    assert!(w.iter().any(|x| x.contains("DNS server")), "the DNS-spoofing caveat is attached to every name rule");
    assert_eq!(crate::validate::normalize_host(" Example.COM. "), "example.com");
}

#[test]
fn nothing_typed_reaches_the_ruleset_text_or_the_kernel() {
    // even a comment (the only free text) is only shown, stripped of control characters, and never sent to the kernel
    let mut c = Config::default();
    c.presets.clear();
    let mut r = rule(Kind::Ip, "198.51.100.7");
    r.comment = "ok\"; drop; \u{7}".into();
    assert!(check_rule(&r).is_err(), "control characters are refused");
    r.comment = "line\nbreak".into();
    assert!(check_rule(&r).is_err());
    r.comment = "say \"hi\"; accept".into();
    assert!(check_rule(&r).is_ok());
    c.add(r);
    let t = text(&c, &Resolved::default(), &Ctx::default());
    for l in t.lines().filter(|l| l.contains("rule 1")) {
        assert!(l.contains("# rule 1: say"), "{l}");
        assert_eq!(l.matches('\n').count(), 0);
    }
    let mut r = rule(Kind::Ip, "198.51.100.7");
    r.comment = "x".repeat(81);
    assert!(check_rule(&r).is_err());
    r = rule(Kind::Ip, "198.51.100.7");
    for bad in ["eth0\"", "eth 0", "eth0;", "a-very-long-interface-name", "eth0\n1"] {
        r.iface = bad.into();
        assert!(check_rule(&r).is_err(), "{bad:?}");
    }
    r.iface = "wlan0".into();
    assert!(check_rule(&r).is_ok());
    // the kernel encoding carries only numbers and the fixed table name
    let (batch, _) = crate::nl::encode_replace(&build(&c, &Resolved::default(), &Ctx::default()).unwrap().ruleset, 1);
    assert!(!batch.windows(3).any(|w| w == b"say"), "the comment never goes to the kernel");
}

#[test]
fn rules_import_and_export_as_json() {
    let mut c = Config::default();
    c.presets = vec![Preset::Ssh, Preset::DnsService];
    let mut a = rule(Kind::Host, "nas.local");
    a.comment = "NAS".into();
    a.expires = 123;
    c.add(a);
    c.add(rule(Kind::Cidr, "192.168.5.0/24"));
    let j = c.to_json();
    let back = Config::from_json(&j).unwrap();
    assert_eq!(back.rules, c.rules);
    assert_eq!(back.presets.len(), 2);
    assert_eq!(back.next_id, 3);
    assert!(Config::from_json("{").unwrap_err().contains("not a firewall export"));
    assert!(Config::from_json(&j.replace("\"version\": 1", "\"version\": 9")).unwrap_err().contains("version 9"));
    assert!(Config::from_json(&j.replace("192.168.5.0/24", "192.168.5.1/24")).unwrap_err().contains("Rule 2"));
    let dup = j.replace("\"id\": 2", "\"id\": 1");
    assert!(Config::from_json(&dup).unwrap_err().contains("twice"));
}

// ------------------------------------------------------------------ the apply / confirm / revert state machine

#[test]
fn a_changed_ruleset_waits_sixty_seconds_for_keep_and_then_comes_back() {
    let mut r = rig();
    r.e.boot(false);
    assert_eq!(r.e.status(), Status::On, "the default is on");
    assert!(r.store.saved.lock().unwrap().is_none(), "defaults are not written until a change is kept");
    let mut c = r.e.config().clone();
    c.add(rule(Kind::Ip, "198.51.100.7"));
    r.e.propose(c, "admin").unwrap();
    assert_eq!(r.e.status(), Status::Pending { secs_left: 60 });
    assert!(r.be.installed().unwrap().to_text().contains("198.51.100.7"), "applied at once so it can be tried");
    assert!(r.store.saved.lock().unwrap().is_none(), "not saved before Keep: a reboot returns to the old rules");
    r.advance(59);
    r.e.tick();
    assert_eq!(r.e.status(), Status::Pending { secs_left: 1 });
    r.advance(1);
    r.e.tick();
    assert_eq!(r.e.status(), Status::On);
    assert!(!r.be.installed().unwrap().to_text().contains("198.51.100.7"), "taken back");
    assert!(r.e.config().rules.is_empty());
    let ev = r.events();
    assert!(ev.contains(&"firewall.apply".to_string()) && ev.contains(&"firewall.revert".to_string()), "{ev:?}");
}

#[test]
fn every_step_of_the_sixty_seconds_is_written_to_the_audit_log_with_who_did_it_whether_the_session_is_local_or_the_remote_desktop() {
    for who in ["admin", "remote desktop: admin"] {
        let mut r = rig();
        r.e.boot(false);
        let first = r.audit.lock().unwrap().len();
        let mut c = r.e.config().clone();
        c.add(rule(Kind::Ip, "198.51.100.7"));
        r.e.propose(c, who).unwrap();
        // the sixty seconds count the same way for a remote session: not kept, so it comes back by itself
        assert_eq!(r.e.status(), Status::Pending { secs_left: 60 }, "{who}");
        r.advance(60);
        r.e.tick();
        assert_eq!(r.e.status(), Status::On, "{who}");
        assert!(r.e.config().rules.is_empty(), "{who}");
        let log: Vec<(String, String)> = r.audit.lock().unwrap()[first..].to_vec();
        let find = |ev: &str| log.iter().find(|(e, _)| e == ev).map(|(_, d)| d.clone());
        assert_eq!(find("firewall.apply").as_deref(), Some(format!("{who}: 1 rules, 1 presets; waiting 60 s for Keep").as_str()), "{log:?}");
        assert!(log.iter().any(|(e, d)| e == "firewall.revert" && d == "not confirmed within 60 seconds"), "the revert says why: {log:?}");
        assert!(find("firewall.keep").is_none(), "{log:?}");
        // the same again, and this time the person keeps it: the keep line names who
        let mut c = r.e.config().clone();
        c.add(rule(Kind::Ip, "198.51.100.8"));
        r.e.propose(c, who).unwrap();
        r.advance(10);
        r.e.tick();
        r.e.keep(who).unwrap();
        let log: Vec<(String, String)> = r.audit.lock().unwrap()[first..].to_vec();
        assert!(log.iter().any(|(e, d)| e == "firewall.keep" && d == who), "{log:?}");
        assert!(r.store.saved.lock().unwrap().is_some(), "{who}");
        // the emergency switch names who pulled it
        r.e.disable_now(who).unwrap();
        let log: Vec<(String, String)> = r.audit.lock().unwrap()[first..].to_vec();
        assert!(log.iter().any(|(e, d)| e == "firewall.disable" && d == &format!("{who}: emergency off")), "{log:?}");
    }
}

#[test]
fn only_a_change_that_lets_more_in_is_recognised_as_opening() {
    let mut r = rig();
    r.e.boot(false);
    let old = r.e.config().clone();
    // a new rule, a new preset, looser pings: all opening
    let mut c = old.clone();
    c.add(rule(Kind::Ip, "198.51.100.7"));
    assert!(c.only_opens_from(&old), "a rule added");
    let mut p = old.clone();
    p.set_preset(Preset::SharedFolders, true);
    assert!(p.only_opens_from(&old), "a preset switched on");
    let mut ping = old.clone();
    ping.ping = Ping::Anyone;
    assert!(ping.only_opens_from(&old), "pings answered for anyone");
    // a rule that stays, with a comment change only
    let mut base = old.clone();
    base.add(rule(Kind::Ip, "198.51.100.7"));
    let mut commented = base.clone();
    commented.rules[0].comment = "my note".into();
    assert!(commented.only_opens_from(&base), "a note changes nothing");
    // tightening: removing a rule, narrowing it, a shorter end time, a preset off, firewall turned on, pings stricter, containers distrusted
    let mut gone = base.clone();
    gone.rules.clear();
    assert!(!gone.only_opens_from(&base), "a rule removed");
    let mut narrow = base.clone();
    narrow.rules[0].ports = "22".into();
    assert!(!narrow.only_opens_from(&base), "a rule narrowed to one port");
    let mut timed = base.clone();
    timed.rules[0].expires = 5000;
    assert!(!timed.only_opens_from(&base), "a rule that now ends");
    let mut off = base.clone();
    off.presets.clear();
    assert!(!off.only_opens_from(&p), "a preset switched off");
    let mut disabled = base.clone();
    disabled.enabled = false;
    assert!(!disabled.only_opens_from(&base) && !base.only_opens_from(&disabled), "turning the firewall off or on is never 'only opens'");
    let mut stricter = old.clone();
    stricter.ping = Ping::Off;
    assert!(!stricter.only_opens_from(&old), "pings stricter than before");
    let mut distrust = old.clone();
    distrust.trust_containers = false;
    assert!(!distrust.only_opens_from(&old), "containers no longer trusted");
    // a longer end time or no end time is opening
    let mut a = old.clone();
    let mut t = rule(Kind::Ip, "198.51.100.9");
    t.expires = 1000;
    a.add(t);
    let mut b = a.clone();
    b.rules[0].expires = 2000;
    assert!(b.only_opens_from(&a), "a longer end time");
    b.rules[0].expires = 0;
    assert!(b.only_opens_from(&a), "no end time at all");
}

#[test]
fn keep_saves_and_revert_by_hand_restores() {
    let mut r = rig();
    r.e.boot(false);
    let mut c = r.e.config().clone();
    c.add(rule(Kind::Ip, "198.51.100.7"));
    r.e.propose(c.clone(), "admin").unwrap();
    r.e.keep("admin").unwrap();
    assert_eq!(r.e.status(), Status::On);
    assert_eq!(r.store.saved.lock().unwrap().as_ref().unwrap().config.rules.len(), 1);
    r.advance(500);
    r.e.tick();
    assert!(r.be.installed().unwrap().to_text().contains("198.51.100.7"), "kept rules stay after the time is up");
    assert!(r.e.keep("admin").is_err(), "nothing left to keep");
    // a second change, reverted by hand
    c.add(rule(Kind::Ip, "198.51.100.8"));
    r.e.propose(c, "admin").unwrap();
    r.e.revert("admin pressed Revert").unwrap();
    assert!(!r.be.installed().unwrap().to_text().contains("198.51.100.8"));
    assert!(r.be.installed().unwrap().to_text().contains("198.51.100.7"), "back to the last kept rules, not to the start");
    // two changes in a row without Keep revert to the last kept state
    let mut c2 = r.e.config().clone();
    c2.add(rule(Kind::Ip, "198.51.100.9"));
    r.e.propose(c2.clone(), "admin").unwrap();
    c2.add(rule(Kind::Ip, "198.51.100.10"));
    r.e.propose(c2, "admin").unwrap();
    r.advance(61);
    r.e.tick();
    let t = r.be.installed().unwrap().to_text();
    assert!(t.contains("198.51.100.7") && !t.contains("198.51.100.9") && !t.contains("198.51.100.10"), "{t}");
}

#[test]
fn a_change_that_is_not_a_change_or_turns_the_firewall_off_needs_no_confirmation() {
    let mut r = rig();
    r.e.boot(false);
    r.e.propose(r.e.config().clone(), "admin").unwrap();
    assert_eq!(r.e.status(), Status::On);
    let mut c = r.e.config().clone();
    c.enabled = false;
    r.e.propose(c, "admin").unwrap();
    assert_eq!(r.e.status(), Status::Off);
    assert!(r.be.installed().is_none(), "our table is gone");
    assert!(r.store.saved.lock().unwrap().as_ref().is_some_and(|s| !s.config.enabled), "saved off");
    // switching it on again is a change that has to be confirmed
    let mut c = r.e.config().clone();
    c.enabled = true;
    r.e.propose(c, "admin").unwrap();
    assert!(matches!(r.e.status(), Status::Pending { .. }));
}

#[test]
fn the_emergency_switch_removes_the_table_and_stays_off_across_a_restart() {
    let store = MemStore::default();
    let mut r = rig_with(store.clone());
    r.e.boot(false);
    let mut c = r.e.config().clone();
    c.add(rule(Kind::Ip, "198.51.100.7"));
    r.e.propose(c, "admin").unwrap();
    r.e.disable_now("admin (emergency key)").unwrap();
    assert_eq!(r.e.status(), Status::Off);
    assert!(r.be.installed().is_none());
    assert!(r.events().contains(&"firewall.disable".to_string()));
    // a new boot reads the saved "off"
    let mut r2 = rig_with(store);
    r2.e.boot(false);
    assert_eq!(r2.e.status(), Status::Off);
    assert!(r2.be.installed().is_none());
}

#[test]
fn the_boot_parameter_skips_the_rules_for_one_boot_only() {
    let store = MemStore::default();
    let mut r = rig_with(store.clone());
    r.e.boot(true);
    assert_eq!(r.e.status(), Status::OffThisBoot);
    assert!(r.be.calls().is_empty(), "nothing is applied");
    assert!(r.events().contains(&"firewall.boot_skipped".to_string()));
    assert_eq!(r.e.status().headline(), "OFF");
    // the next boot without the parameter applies again
    let mut r2 = rig_with(store);
    r2.e.boot(false);
    assert_eq!(r2.e.status(), Status::On);
}

#[test]
fn a_failure_to_apply_is_loud_and_never_pretends() {
    // at boot
    let mut r = rig();
    r.be.0.lock().unwrap().fail_replace = Some(BackendError::Failed("the kernel refused the firewall rules (error 22)".into()));
    r.e.boot(false);
    let s = r.e.status();
    assert!(matches!(s, Status::NotProtected(_)), "{s:?}");
    assert_eq!(s.headline(), "NOT PROTECTED");
    assert!(!s.protected());
    assert!(r.events().contains(&"firewall.apply_failed".to_string()));
    assert!(r.e.view().last_error.unwrap().contains("refused"));
    // no nf_tables in the kernel: said plainly
    let mut r = rig();
    r.be.0.lock().unwrap().fail_replace = Some(BackendError::Unavailable("Firewall not available in this kernel".into()));
    r.e.boot(false);
    assert_eq!(r.e.status(), Status::Unavailable("Firewall not available in this kernel".into()));
    assert_eq!(r.e.status().headline(), "NOT PROTECTED");
    // later it works (modules were loaded): the status recovers
    r.be.0.lock().unwrap().fail_replace = None;
    r.e.network_changed();
    assert_eq!(r.e.status(), Status::On);
    // a failed change keeps the old rules in force (the kernel change is atomic) and says so
    let mut r = rig();
    r.e.boot(false);
    r.be.0.lock().unwrap().fail_replace = Some(BackendError::Failed("boom".into()));
    let mut c = r.e.config().clone();
    c.add(rule(Kind::Ip, "198.51.100.7"));
    assert!(r.e.propose(c, "admin").is_err());
    assert_eq!(r.e.status(), Status::On, "the previous rules are still in the kernel");
    assert!(r.e.config().rules.is_empty());
    assert_eq!(r.e.view().last_error.as_deref(), Some("boom"));
}

#[test]
fn a_revert_that_fails_says_not_protected() {
    let mut r = rig();
    r.e.boot(false);
    let mut c = r.e.config().clone();
    c.add(rule(Kind::Ip, "198.51.100.7"));
    r.e.propose(c, "admin").unwrap();
    r.be.0.lock().unwrap().fail_replace = Some(BackendError::Failed("boom".into()));
    r.advance(61);
    r.e.tick();
    // the kernel still holds the unconfirmed rules; the page says the revert failed
    assert!(r.events().contains(&"firewall.apply_failed".to_string()));
    assert!(r.e.view().last_error.is_some());
}

#[test]
fn invalid_settings_are_refused_before_anything_is_applied() {
    let mut r = rig();
    r.e.boot(false);
    let before = r.be.calls().len();
    let mut c = r.e.config().clone();
    c.add(rule(Kind::Cidr, "10.0.0.5/24"));
    assert!(r.e.propose(c, "admin").unwrap_err().contains("10.0.0.0"));
    assert_eq!(r.be.calls().len(), before, "the kernel was not called");
    assert_eq!(r.e.status(), Status::On);
}

#[test]
fn unreadable_saved_settings_fall_back_to_blocking_everything() {
    let store = MemStore::default();
    *store.fail_load.lock().unwrap() = true;
    let mut r = rig_with(store);
    r.e.boot(false);
    assert_eq!(r.e.status(), Status::On, "the safe default is on");
    assert!(r.events().contains(&"firewall.settings_unreadable".to_string()));
}

#[test]
fn host_names_are_resolved_refreshed_and_keep_their_last_good_answer() {
    let mut r = rig();
    r.dns.set("nas.local", &["192.168.1.9"]);
    r.e.boot(false);
    let mut c = r.e.config().clone();
    c.add(rule(Kind::Host, "NAS.local"));
    r.e.propose(c, "admin").unwrap();
    r.e.keep("admin").unwrap();
    assert!(r.be.installed().unwrap().to_text().contains("ip saddr 192.168.1.9 counter accept"));
    assert_eq!(r.e.view().hosts[0].addrs, vec![ip("192.168.1.9")]);
    assert!(r.store.saved.lock().unwrap().as_ref().unwrap().resolved.addrs.contains_key("nas.local"), "the answer is saved for the next boot");
    // not due yet: nothing changes
    r.dns.set("nas.local", &["192.168.1.10"]);
    r.advance(299);
    r.e.tick();
    assert!(r.be.installed().unwrap().to_text().contains("192.168.1.9"));
    // five minutes: the new answer is applied, the old one is gone
    r.advance(1);
    r.e.tick();
    let t = r.be.installed().unwrap().to_text();
    assert!(t.contains("192.168.1.10") && !t.contains("192.168.1.9"), "{t}");
    assert!(r.events().contains(&"firewall.dns".to_string()));
    // the lookup fails: the last good set stays and there is a warning
    r.dns.fail("nas.local");
    r.advance(300);
    r.e.tick();
    assert!(r.be.installed().unwrap().to_text().contains("192.168.1.10"));
    let h = &r.e.view().hosts[0];
    assert!(h.warning.as_ref().is_some_and(|w| w.contains("last good")), "{h:?}");
    assert_eq!(h.addrs, vec![ip("192.168.1.10")]);
    // the network changing triggers a lookup at the next opportunity
    r.dns.set("nas.local", &["192.168.1.11", "2001:db8::11"]);
    r.e.network_changed();
    r.e.tick();
    let t = r.be.installed().unwrap().to_text();
    assert!(t.contains("192.168.1.11") && t.contains("2001:db8::11"), "{t}");
    assert!(r.e.view().hosts[0].warning.is_none(), "the warning goes away with a good answer");
    // a rule that is deleted forgets its name
    let mut c = r.e.config().clone();
    c.rules.clear();
    r.e.propose(c, "admin").unwrap();
    r.e.refresh_dns(true);
    assert!(r.e.resolved().addrs.is_empty());
}

#[test]
fn a_name_that_never_resolved_allows_nothing_and_says_so() {
    let mut r = rig();
    r.e.boot(false);
    let mut c = r.e.config().clone();
    c.add(rule(Kind::Host, "ghost.example"));
    r.e.propose(c, "admin").unwrap();
    let v = r.e.view();
    assert_eq!(v.rule_states[0].1, RuleState::WaitingForDns);
    assert!(v.hosts[0].warning.as_ref().unwrap().contains("allows nothing"));
    assert!(!r.be.installed().unwrap().to_text().contains("ghost"));
}

#[test]
fn rules_with_an_end_time_stop_working_on_time() {
    let mut r = rig();
    r.e.boot(false);
    let mut c = r.e.config().clone();
    let mut a = rule(Kind::Ip, "198.51.100.7");
    a.expires = r.clock.now() + 3600;
    let id = c.add(a);
    r.e.propose(c, "admin").unwrap();
    r.e.keep("admin").unwrap();
    r.advance(3599);
    r.e.tick();
    assert!(r.be.installed().unwrap().to_text().contains("198.51.100.7"));
    r.advance(1);
    r.e.tick();
    assert!(!r.be.installed().unwrap().to_text().contains("198.51.100.7"), "ended");
    assert!(r.events().contains(&"firewall.expire".to_string()));
    assert_eq!(r.e.view().rule_states.iter().find(|(i, _)| *i == id).unwrap().1, RuleState::Expired);
}

#[test]
fn a_change_that_cuts_off_the_remote_desktop_is_taken_back_after_sixty_seconds_without_anyone_pressing_anything_and_every_step_is_audited() {
    let mut r = rig();
    r.e.boot(false);
    let lan = ip("192.168.1.20");
    // the person works through the remote desktop: its preset is kept, the web port is reachable
    assert!(r.e.would_allow_tcp(lan, crate::ruleset::REMOTE_PORT), "the default keeps the remote desktop reachable");
    let mut c = r.e.config().clone();
    c.set_preset(Preset::RemoteDesktop, true);
    r.e.propose(c, "admin (remote desktop)").unwrap();
    assert_eq!(r.e.status(), Status::On, "no change: nothing to confirm");
    // then a change over that very connection switches the preset off: the connection is gone, nobody can press Keep
    let mut c = r.e.config().clone();
    c.set_preset(Preset::RemoteDesktop, false);
    r.e.propose(c, "admin (remote desktop)").unwrap();
    assert!(!r.e.would_allow_tcp(lan, crate::ruleset::REMOTE_PORT), "applied at once, so the cut-off is real while it waits");
    assert_eq!(r.e.status(), Status::Pending { secs_left: 60 });
    r.advance(59);
    r.e.tick();
    assert!(!r.e.would_allow_tcp(lan, crate::ruleset::REMOTE_PORT));
    r.advance(1);
    r.e.tick();
    assert!(r.e.would_allow_tcp(lan, crate::ruleset::REMOTE_PORT), "the kept rules are back by themselves: the remote desktop can be reached again");
    assert_eq!(r.e.status(), Status::On);
    // the audit log names who did what, in order
    let log = r.audit.lock().unwrap().clone();
    let kinds: Vec<&str> = log.iter().map(|(e, _)| e.as_str()).collect();
    let pos = |ev: &str| kinds.iter().rposition(|k| *k == ev).unwrap_or_else(|| panic!("no {ev} in {kinds:?}"));
    assert!(pos("firewall.apply") < pos("firewall.revert"));
    let applied = log.iter().rev().find(|(e, d)| e == "firewall.apply" && d.contains("Keep")).map(|(_, d)| d.clone()).expect("the change that waited for Keep");
    assert!(applied.starts_with("admin (remote desktop):") && applied.contains("waiting 60 s for Keep"), "{applied}");
    // a kept change is audited with its person as well
    let mut c = r.e.config().clone();
    c.add(rule(Kind::Ip, "198.51.100.7"));
    r.e.propose(c, "admin").unwrap();
    r.e.keep("admin").unwrap();
    assert!(r.audit.lock().unwrap().iter().any(|(e, d)| e == "firewall.keep" && d == "admin"));
    assert_eq!(log.iter().rev().find(|(e, _)| e == "firewall.revert").map(|(_, d)| d.as_str()), Some("not confirmed within 60 seconds"));
    // the emergency switch is audited with its person too
    r.e.disable_now("root").unwrap();
    assert!(r.audit.lock().unwrap().iter().any(|(e, d)| e == "firewall.disable" && d == "root: emergency off"));
}

#[test]
fn a_share_port_the_firewall_blocks_is_found_out() {
    let mut r = rig();
    r.e.boot(false);
    let lan = ip("192.168.1.20");
    assert!(!r.e.would_allow_tcp(lan, 8081), "blocked by default");
    let mut c = r.e.config().clone();
    c.set_preset(Preset::SharedFolders, true);
    r.e.propose(c, "admin").unwrap();
    assert!(r.e.would_allow_tcp(lan, 8081));
    r.e.disable_now("admin").unwrap();
    assert!(r.e.would_allow_tcp(lan, 8081), "no firewall in force: nothing is blocked");
}

#[test]
fn the_media_server_preset_opens_its_tcp_range_and_the_finding_port_to_the_local_network_only() {
    use crate::ruleset::{eval, Packet, MEDIA_PORTS, SSDP_PORT};
    let mut r = rig();
    r.e.boot(false);
    let (lan, far) = (ip("192.168.1.20"), ip("8.8.8.8"));
    assert!(!r.e.would_allow_tcp(lan, MEDIA_PORTS.0), "blocked by default");
    let mut c = r.e.config().clone();
    c.set_preset(Preset::MediaServer, true);
    r.e.propose(c, "admin").unwrap();
    assert_eq!(Preset::from_id("media"), Some(Preset::MediaServer));
    assert!(r.e.would_allow_tcp(lan, MEDIA_PORTS.0) && r.e.would_allow_tcp(lan, MEDIA_PORTS.1));
    assert!(!r.e.would_allow_tcp(lan, MEDIA_PORTS.1 + 1) && !r.e.would_allow_tcp(lan, MEDIA_PORTS.0 - 1), "only the range");
    assert!(!r.e.would_allow_tcp(far, MEDIA_PORTS.0), "the internet never reaches it");
    let b = crate::ruleset::build(r.e.config(), r.e.resolved(), &crate::ruleset::Ctx::default()).unwrap();
    let udp = |src, port| Packet { iif: "eth0".into(), ct: crate::ruleset::CT_NEW, src, l4: 17, dport: port, icmp_type: 0 };
    assert!(eval(&b.ruleset, &udp(lan, SSDP_PORT)), "players find it by UDP 1900");
    assert!(!eval(&b.ruleset, &udp(far, SSDP_PORT)));
    assert!(!eval(&b.ruleset, &udp(lan, MEDIA_PORTS.0)), "the playing ports are TCP only");
}

#[test]
fn counters_are_read_only_while_the_table_is_in_the_kernel() {
    let mut r = rig();
    assert_eq!(r.e.counters(), None);
    r.e.boot(false);
    r.be.0.lock().unwrap().packets = 7;
    assert_eq!(r.e.counters(), Some((7, 420)));
}

// ------------------------------------------------------------------ the kernel messages

#[test]
fn the_netlink_batch_is_well_formed() {
    let rs = default_rs();
    let (b, acks) = crate::nl::encode_replace(&rs, 100);
    let msgs = crate::nl::split_messages(&b);
    assert_eq!(msgs.first().unwrap().0, 0x10, "batch begin");
    assert_eq!(msgs.last().unwrap().0, 0x11, "batch end");
    // table (create), table (delete), table (create), chain, then one message per rule
    assert_eq!(msgs.len(), 2 + 3 + 1 + rs.rules.len());
    assert_eq!(acks, 3 + 1 + rs.rules.len());
    let types: Vec<u16> = msgs.iter().map(|m| m.0).collect();
    assert_eq!(&types[1..6], &[0xa00, 0xa02, 0xa00, 0xa03, 0xa06], "NEWTABLE DELTABLE NEWTABLE NEWCHAIN NEWRULE");
    // every length field is consistent and 4-byte aligned
    let mut i = 0;
    while i < b.len() {
        let l = u32::from_ne_bytes(b[i..i + 4].try_into().unwrap()) as usize;
        assert!(l >= 16 && i + l <= b.len());
        i += (l + 3) & !3;
    }
    assert_eq!(i, b.len());
    // the chain is a filter chain on the input hook with the drop policy
    let chain = msgs[4].1;
    let a = crate::nl::attrs(&chain[4..]);
    assert_eq!(a.iter().find(|x| x.0 == 5).unwrap().1, &[0, 0, 0, 0], "policy drop");
    assert_eq!(a.iter().find(|x| x.0 == 7).unwrap().1, b"filter\0");
    let hook = crate::nl::attrs(a.iter().find(|x| x.0 == 4).unwrap().1);
    assert_eq!(hook[0].1, &[0, 0, 0, 1], "NF_INET_LOCAL_IN");
    assert_eq!(hook[1].1, &[0, 0, 0, 0], "priority 0");
    // removing touches only our table
    let (r, n) = crate::nl::encode_remove(5);
    let m = crate::nl::split_messages(&r);
    assert_eq!((n, m.len()), (2, 4));
    assert!(m[1..3].iter().all(|x| x.1.windows(10).any(|w| w == b"iyakki_fw\0")));
}

#[test]
fn cidr_masks_are_exact() {
    let m = |l: u8, n: usize| {
        // through the public encoder: a /l source must produce a bitwise mask of l bits
        let rs = crate::ruleset::Ruleset { rules: vec![crate::ruleset::RuleSpec { note: String::new(), steps: vec![crate::ruleset::Step::Saddr(crate::ruleset::Addr::Cidr(if n == 4 { ip("10.0.0.0") } else { ip("2001:db8::") }, l)), crate::ruleset::Step::Accept] }], sets: Vec::new() };
        crate::nl::encode_replace(&rs, 1).0
    };
    let b = m(13, 4);
    assert!(b.windows(4).any(|w| w == [0xff, 0xf8, 0, 0]), "/13 -> 255.248.0.0");
    let b = m(48, 16);
    assert!(b.windows(8).any(|w| w == [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0]));
    // /0 matches everything: no address expression at all
    let b = m(0, 4);
    assert!(!b.windows(7).any(|w| w == b"payload"));
}

#[test]
fn counters_are_read_out_of_a_dump_reply() {
    // build a NEWRULE reply by hand: nfgenmsg + rule attrs { expressions { elem { name "counter", data { bytes, packets } } } }
    fn attr(t: u16, d: &[u8]) -> Vec<u8> {
        let mut v = ((4 + d.len()) as u16).to_ne_bytes().to_vec();
        v.extend(t.to_ne_bytes());
        v.extend(d);
        while v.len() % 4 != 0 {
            v.push(0);
        }
        v
    }
    let data = [attr(1, &500u64.to_be_bytes()), attr(2, &9u64.to_be_bytes())].concat();
    let elem = [attr(1, b"counter\0"), attr(2, &data)].concat();
    let exprs = attr(4, &attr(1, &elem));
    let mut payload = vec![1, 0, 0, 0];
    payload.extend(exprs);
    let mut m = vec![0u8; 16];
    m[4..6].copy_from_slice(&0xa06u16.to_ne_bytes());
    m.extend(payload);
    let l = m.len() as u32;
    m[0..4].copy_from_slice(&l.to_ne_bytes());
    assert_eq!(crate::nl::counters_of_dump(&m), (9, 500));
    assert_eq!(crate::nl::counters_of_dump(&[]), (0, 0));
}

#[test]
fn kernel_error_numbers_become_plain_words() {
    use crate::nl::{errno_error, NlError};
    assert_eq!(errno_error(libc::EOPNOTSUPP), NlError::Unavailable("Firewall not available in this kernel".into()));
    assert_eq!(errno_error(libc::EPROTONOSUPPORT), NlError::Unavailable("Firewall not available in this kernel".into()));
    assert!(matches!(errno_error(libc::EPERM), NlError::Failed(m) if m.contains("administrator")));
    assert!(matches!(errno_error(libc::EINVAL), NlError::Failed(m) if m.contains("22")));
}

// ------------------------------------------------------------------ the blocked log

#[test]
fn blocked_packets_are_read_into_a_ring_of_fifty() {
    let mut v4 = vec![0u8; 24];
    v4[0] = 0x45;
    v4[9] = 6;
    v4[12..16].copy_from_slice(&[203, 0, 113, 9]);
    v4[22..24].copy_from_slice(&9000u16.to_be_bytes());
    assert_eq!(blocked::parse_packet(&v4), Some((ip("203.0.113.9"), 6, 9000)));
    let mut v6 = vec![0u8; 44];
    v6[0] = 0x60;
    v6[6] = 17;
    v6[8..24].copy_from_slice(&"2001:db8::9".parse::<std::net::Ipv6Addr>().unwrap().octets());
    v6[42..44].copy_from_slice(&53u16.to_be_bytes());
    assert_eq!(blocked::parse_packet(&v6), Some((ip("2001:db8::9"), 17, 53)));
    v4[9] = 1;
    assert_eq!(blocked::parse_packet(&v4).unwrap().2, 0, "icmp has no port");
    assert_eq!(blocked::parse_packet(&[]), None);
    assert_eq!(blocked::parse_packet(&[0x45, 0, 0]), None);
    assert_eq!(blocked::parse_packet(&[0x30; 40]), None);
    let log = BlockLog::new();
    for i in 0..80u16 {
        log.push(Blocked { at: 1000 + i64::from(i) * 100, src: ip("203.0.113.9"), proto: 6, dport: i + 1, count: 1 });
    }
    let r = log.recent();
    assert_eq!(r.len(), blocked::RING);
    assert_eq!(r[0].dport, 80, "newest first");
    assert_eq!(r[49].dport, 31);
    // a client retrying the same connection is one line
    let log = BlockLog::new();
    for t in 0..4 {
        log.push(Blocked { at: 5 + t, src: ip("203.0.113.9"), proto: 6, dport: 22, count: 1 });
    }
    assert_eq!(log.recent().len(), 1);
    assert_eq!(log.recent()[0].count, 4);
    log.clear();
    assert!(log.recent().is_empty());
}

// ------------------------------------------------------------------ the real service's helpers

#[test]
fn the_kernel_command_line_is_read() {
    use crate::service::{ctx_from, is_cloud, param, web_port};
    let c = "console=ttyS0 iyakki.ssh=1 iyakki.firewall=off iyakki.web=9443 iyakki.mode=cloud";
    assert_eq!(param(c, "iyakki.firewall"), Some("off"));
    assert_eq!(param(c, "iyakki.fire"), None);
    assert_eq!(param("iyakki.firewall=on", "iyakki.firewall"), Some("on"));
    assert_eq!(web_port("9443"), Some(9443));
    assert_eq!(web_port("http:8080"), Some(8080));
    assert_eq!(web_port("0.0.0.0:7000"), Some(7000));
    assert_eq!(web_port("1"), None);
    assert_eq!(web_port("off"), None);
    let x = ctx_from(c, "", true);
    assert!(x.ssh_boot && x.cloud);
    assert_eq!(x.remote_port, Some(9443));
    let y = ctx_from("quiet", "NAME=IYAKKI\nVARIANT=cloud-x86_64\n", false);
    assert!(y.cloud && !y.ssh_boot && !y.ipv6);
    assert!(!is_cloud("quiet", "VARIANT=desktop"));
}

#[test]
fn the_settings_file_is_written_atomically_and_a_broken_file_is_set_aside() {
    use crate::service::FileStore;
    let dir = std::env::temp_dir().join(format!("iyakki-fw-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let st = FileStore { path: dir.join("sub/firewall.json") };
    assert_eq!(st.load().unwrap().map(|_| ()), None, "nothing saved yet");
    let mut c = Config::default();
    c.add(rule(Kind::Ip, "198.51.100.7"));
    st.save(&Saved { config: c.clone(), resolved: Resolved::default() }).unwrap();
    assert_eq!(st.load().unwrap().unwrap().config, c);
    assert!(!dir.join("sub/firewall.json.tmp").exists());
    std::fs::write(&st.path, "{ not json").unwrap();
    assert!(st.load().is_err());
    assert!(dir.join("sub/firewall.json.bad").exists(), "kept for inspection");
    assert_eq!(st.load().unwrap().map(|_| ()), None, "and the next start begins with the defaults");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_emergency_command_removes_the_table_saves_off_and_leaves_a_note() {
    use crate::service::{emergency_off_with, FileStore};
    let dir = std::env::temp_dir().join(format!("iyakki-fw-emerg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = FileStore { path: dir.join("firewall.json") };
    let mut c = Config::default();
    c.add(rule(Kind::Ip, "198.51.100.7"));
    st.save(&Saved { config: c, resolved: Resolved::default() }).unwrap();
    let mut be = FakeBackend::default();
    let msg = emergency_off_with(&st.path, &dir.join("note"), &mut be).unwrap();
    assert!(msg.contains("stays off after a restart"), "{msg}");
    assert_eq!(be.calls(), vec!["remove"]);
    let after = st.load().unwrap().unwrap();
    assert!(!after.config.enabled && after.config.rules.len() == 1, "off, and the rules are kept for later");
    assert!(dir.join("note").exists(), "the running firewall is told");
    // with nothing saved yet it still works
    let st2 = FileStore { path: dir.join("none.json") };
    assert!(emergency_off_with(&st2.path, &dir.join("note2"), &mut FakeBackend::default()).is_ok());
    assert!(!st2.load().unwrap().unwrap().config.enabled);
    // a kernel that refuses is an error, not a pretend success
    let mut bad = FakeBackend::default();
    bad.0.lock().unwrap().fail_remove = Some(BackendError::Failed("boom".into()));
    assert!(emergency_off_with(&st.path, &dir.join("note3"), &mut bad).unwrap_err().contains("boom"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ direct links (one cable between two computers)

fn direct_ctx(names: &[&str], more: bool) -> Ctx {
    Ctx { direct_ifaces: names.iter().map(|s| s.to_string()).collect(), direct_allow_more: more, ..Ctx::default() }
}

fn pkt(iif: &str, src: &str, l4: u8, dport: u16) -> crate::ruleset::Packet {
    crate::ruleset::Packet { iif: iif.into(), ct: crate::ruleset::CT_NEW, src: src.parse().unwrap(), l4, dport, icmp_type: 8 }
}

#[test]
fn a_direct_link_lets_in_only_developer_mode_the_beacon_and_ping_by_default() {
    let b = build(&Config::default(), &Resolved::default(), &direct_ctx(&["thunderbolt0"], false)).unwrap();
    let rs = &b.ruleset;
    assert!(crate::ruleset::eval(rs, &pkt("thunderbolt0", "169.254.7.7", 6, 7979)), "developer mode");
    assert!(crate::ruleset::eval(rs, &pkt("thunderbolt0", "fe80::1", 6, 7979)), "developer mode over IPv6");
    assert!(crate::ruleset::eval(rs, &pkt("thunderbolt0", "169.254.7.7", 17, 7980)), "the beacon");
    assert!(crate::ruleset::eval(rs, &pkt("thunderbolt0", "169.254.7.7", 1, 0)), "ping");
    assert!(crate::ruleset::eval(rs, &pkt("thunderbolt0", "fe80::1", 58, 0)), "ping over IPv6");
    for (l4, port) in [(6u8, 22u16), (6, 8443), (6, 6443), (17, 53), (6, 80), (17, 7979), (6, 7980)] {
        assert!(!crate::ruleset::eval(rs, &pkt("thunderbolt0", "169.254.7.7", l4, port)), "{l4}/{port} must stay blocked on a direct link");
    }
    // not on any other interface
    assert!(!crate::ruleset::eval(rs, &pkt("wlan0", "8.8.8.8", 6, 7979)));
    assert!(!crate::ruleset::eval(rs, &pkt("eth0", "169.254.7.7", 6, 7979)));
}

#[test]
fn the_allow_more_switch_lets_everything_in_on_the_direct_link_only() {
    let b = build(&Config::default(), &Resolved::default(), &direct_ctx(&["usb0"], true)).unwrap();
    assert!(crate::ruleset::eval(&b.ruleset, &pkt("usb0", "10.77.0.2", 6, 22)));
    assert!(!crate::ruleset::eval(&b.ruleset, &pkt("eth0", "10.77.0.2", 6, 22)));
    assert!(b.ruleset.to_text().contains("direct link: everything allowed"));
}

#[test]
fn no_direct_link_means_no_direct_rules_and_a_made_up_name_never_reaches_a_rule() {
    let plain = build(&Config::default(), &Resolved::default(), &Ctx::default()).unwrap().ruleset.to_text();
    assert!(!plain.contains("direct link"));
    let t = build(&Config::default(), &Resolved::default(), &direct_ctx(&["", "bad name", "a;b", "waytoolonginterfacename", "enx001122334455"], false)).unwrap().ruleset.to_text();
    assert!(t.contains("enx001122334455") && !t.contains("bad name") && !t.contains("a;b") && !t.contains("waytoolong"), "{t}");
    // the default rules are unchanged by the zone: the same text with a zone only adds lines
    assert!(t.lines().filter(|l| l.contains("direct link")).count() == 5);
}

// ------------------------------------------------------------------ internet sharing zone

#[test]
fn the_sharing_zone_lets_in_only_dhcp_and_dns_on_the_shared_interface() {
    let ctx = Ctx { share_ifaces: vec!["enx00e04c680001".into()], ..Ctx::default() };
    let b = build(&Config::default(), &Resolved::default(), &ctx).unwrap();
    let rs = &b.ruleset;
    let i = "enx00e04c680001";
    assert!(crate::ruleset::eval(rs, &pkt(i, "10.42.0.10", 17, 67)), "DHCP");
    assert!(crate::ruleset::eval(rs, &pkt(i, "10.42.0.10", 17, 53)), "DNS");
    for (l4, port) in [(6u8, 22u16), (6, 53), (6, 7979), (6, 8443), (17, 5353), (17, 7980), (1, 0)] {
        assert!(!crate::ruleset::eval(rs, &pkt(i, "10.42.0.10", l4, port)), "{l4}/{port} must stay blocked for a device that uses the shared internet");
    }
    assert!(!crate::ruleset::eval(rs, &pkt("eth0", "10.42.0.10", 17, 67)), "only on the shared interface");
    let none = build(&Config::default(), &Resolved::default(), &Ctx { share_ifaces: vec!["a;b".into()], ..Ctx::default() }).unwrap();
    assert!(!none.ruleset.to_text().contains("sharing:"), "a made-up name never reaches a rule");
}

// ------------------------------------------------------------------ per-rule counters and sets for long lists (F-FW-10)

mod counters_and_sets {
    use super::*;
    use crate::nl;
    use crate::ruleset::{Addr, build_with, eval_counting, rule_counts, rule_id_of_note, Ruleset, Step, BLOCKED_NOTE, SET_MIN};

    fn attr(t: u16, d: &[u8]) -> Vec<u8> {
        let mut v = ((4 + d.len()) as u16).to_ne_bytes().to_vec();
        v.extend(t.to_ne_bytes());
        v.extend(d);
        while v.len() % 4 != 0 {
            v.push(0);
        }
        v
    }

    /// A dump reply the way the kernel lists the rules: one NEWRULE message per rule, a counter expression where there is a count.
    fn dump_reply(per: &[Option<(u64, u64)>]) -> Vec<u8> {
        let mut out = Vec::new();
        for c in per {
            let elem = match c {
                Some((pk, by)) => {
                    let data = [attr(1, &by.to_be_bytes()), attr(2, &pk.to_be_bytes())].concat();
                    [attr(1, b"counter\0"), attr(2, &data)].concat()
                }
                None => [attr(1, b"immediate\0"), attr(2, &[0; 4])].concat(),
            };
            let mut payload = vec![1, 0, 0, 0];
            payload.extend(attr(4, &attr(1, &elem)));
            let mut m = vec![0u8; 16];
            m[4..6].copy_from_slice(&0xa06u16.to_ne_bytes());
            m.extend(payload);
            let l = m.len() as u32;
            m[0..4].copy_from_slice(&l.to_ne_bytes());
            out.extend(m);
        }
        out
    }

    fn expr_names(rule_payload: &[u8]) -> Vec<String> {
        let list = nl::attrs(&rule_payload[4..]).into_iter().find(|(t, _)| *t == 4).map(|(_, v)| v).unwrap_or(&[]);
        nl::attrs(list)
            .into_iter()
            .map(|(_, e)| {
                let a = nl::attrs(e);
                let n = a.iter().find(|(t, _)| *t == 1).map(|(_, v)| *v).unwrap_or(&[]);
                String::from_utf8_lossy(n.split(|b| *b == 0).next().unwrap_or(&[])).into_owned()
            })
            .collect()
    }

    fn a_config() -> Config {
        let mut c = Config::default();
        c.presets.clear();
        c.set_preset(Preset::Ssh, true);
        let mut a = rule(Kind::Ip, "198.51.100.7");
        a.proto = Proto::Tcp;
        a.ports = "8080".into();
        c.add(a);
        let mut b = rule(Kind::Cidr, "203.0.113.0/24");
        b.comment = "office".into();
        c.add(b);
        c
    }

    #[test]
    fn rules_of_the_page_and_presets_count_what_they_let_in_and_the_blocked_counter_stays_last() {
        let rs = build(&a_config(), &Resolved::default(), &Ctx::default()).unwrap().ruleset;
        let mut counted = 0;
        for r in &rs.rules {
            let has = r.steps.contains(&Step::Counter);
            let mine = r.note.starts_with("rule ") || r.note.starts_with("preset: ");
            if mine {
                let n = r.steps.len();
                assert_eq!(r.steps[n - 2], Step::Counter, "the counter sits right before the verdict: {}", r.note);
                assert_eq!(r.steps[n - 1], Step::Accept);
                counted += 1;
            } else if r.note != BLOCKED_NOTE {
                assert!(!has, "only the rules of the page and the presets are counted, not {}", r.note);
            }
        }
        assert!(counted >= 8, "two rules and the SSH preset on every local network: {counted}");
        let last_counted = rs.rules.iter().rev().find(|r| r.steps.contains(&Step::Counter)).unwrap();
        assert_eq!(last_counted.note, BLOCKED_NOTE, "counters_of_dump reads the blocked total from the last counted rule");
    }

    #[test]
    fn what_the_kernel_would_report_per_rule_matches_what_the_evaluator_counts_and_the_blocked_total_stays_apart() {
        let rs = build(&a_config(), &Resolved::default(), &Ctx::default()).unwrap().ruleset;
        let mut hits: Vec<u64> = Vec::new();
        let sources = [("198.51.100.7", 8080, 3), ("198.51.100.7", 9999, 2), ("203.0.113.9", 443, 4), ("192.168.1.5", 22, 5), ("8.8.8.8", 22, 6)];
        for (src, port, n) in sources {
            for _ in 0..n {
                eval_counting(&rs, &Packet::new_tcp("eth0", ip(src), port), &mut hits);
            }
        }
        let per: Vec<Option<(u64, u64)>> = rs.rules.iter().enumerate().map(|(i, r)| r.steps.contains(&Step::Counter).then(|| (hits[i], hits[i] * 60))).collect();
        let reply = dump_reply(&per);
        assert_eq!(nl::rule_counters_of_dump(&reply), per, "the reader gives back one entry per rule, in order");
        let counts = rule_counts(&rs, &nl::rule_counters_of_dump(&reply));
        let of = |note: &str| counts.iter().find(|c| c.note == note).map(|c| c.packets);
        assert_eq!(of("rule 1"), Some(3), "only the port 8080 connections from the one address");
        assert_eq!(of("rule 2: office"), Some(4));
        assert_eq!(of("preset: SSH"), Some(5), "the six local networks of the preset add up under one note");
        assert!(counts.iter().all(|c| c.note != BLOCKED_NOTE));
        assert_eq!(rule_id_of_note("rule 2: office"), Some(2));
        assert_eq!(rule_id_of_note("rule 14"), Some(14));
        assert_eq!(rule_id_of_note("preset: SSH"), None);
        // blocked: 2 (wrong port) + 6 (outside address) new connections reach the blocked counter and nothing else is mixed in
        assert_eq!(nl::counters_of_dump(&reply), (8, 480));
        // a list of another length (the table changed since) gives nothing instead of a wrong number
        assert!(rule_counts(&rs, &per[1..]).is_empty());
    }

    #[test]
    fn the_batch_for_the_kernel_has_a_counter_expression_in_exactly_the_rules_that_have_the_step() {
        let rs = build(&a_config(), &Resolved::default(), &Ctx::default()).unwrap().ruleset;
        let (bytes, acks) = nl::encode_replace(&rs, 10);
        let rules: Vec<Vec<String>> = nl::split_messages(&bytes).into_iter().filter(|(t, _)| t & 0xff == 6 && t >> 8 == 10).map(|(_, p)| expr_names(p)).collect();
        assert_eq!(rules.len(), rs.rules.len());
        for (names, r) in rules.iter().zip(&rs.rules) {
            assert_eq!(names.iter().filter(|n| *n == "counter").count(), r.steps.iter().filter(|s| **s == Step::Counter).count(), "{}", r.note);
            if r.steps.contains(&Step::Counter) && r.steps.last() == Some(&Step::Accept) {
                let at = names.iter().position(|n| n == "counter").unwrap();
                assert_eq!(names[at + 1], "immediate", "the verdict follows the counter: {}", r.note);
            }
        }
        assert_eq!(acks, 3 + 1 + rs.rules.len(), "table x3, chain, one per rule");
    }

    fn many(n: u8) -> Vec<IpAddr> {
        (1..=n).map(|i| ip(&format!("198.51.100.{i}"))).collect()
    }

    fn host_config() -> (Config, Resolved) {
        let mut c = Config::default();
        c.presets.clear();
        let mut h = rule(Kind::Host, "big.example.com");
        h.proto = Proto::Tcp;
        h.ports = "22".into();
        c.add(h);
        let mut r = Resolved::default();
        let mut addrs = many(40);
        addrs.extend([ip("2001:db8::1"), ip("2001:db8::2"), ip("2001:db8::3")]);
        r.addrs.insert("big.example.com".into(), addrs);
        (c, r)
    }

    #[test]
    fn a_host_with_many_answers_is_one_rule_and_a_set_and_every_verdict_stays_the_same() {
        let (c, r) = host_config();
        let flat = build_with(&c, &r, &Ctx::default(), usize::MAX).unwrap().ruleset;
        let set = build(&c, &r, &Ctx::default()).unwrap().ruleset;
        assert!(flat.sets.is_empty());
        assert_eq!(set.sets.len(), 1, "the forty IPv4 answers; the three IPv6 answers stay as rules");
        assert_eq!(set.sets[0].members.len(), 40);
        assert!(set.sets[0].v4);
        assert_eq!(set.rules.len() + 39, flat.rules.len(), "forty rules became one");
        let t = set.to_text();
        assert!(t.contains("set l1 {") && t.contains("type ipv4_addr") && t.contains("saddr @l1 meta l4proto tcp th dport 22 counter accept  # rule 1"), "{t}");
        let mut hits_flat = Vec::new();
        let mut hits_set = Vec::new();
        for last in 0..=60u8 {
            for src in [format!("198.51.100.{last}"), format!("203.0.113.{last}"), format!("2001:db8::{last:x}")] {
                for port in [22u16, 23, 8080] {
                    let p = Packet::new_tcp("eth0", ip(&src), port);
                    assert_eq!(eval(&flat, &p), eval(&set, &p), "{src} -> {port}");
                    eval_counting(&flat, &p, &mut hits_flat);
                    eval_counting(&set, &p, &mut hits_set);
                }
            }
        }
        let sum = |rs: &Ruleset, h: &[u64], note: &str| -> u64 { rs.rules.iter().zip(h).filter(|(r, _)| r.note == note).map(|(_, n)| n).sum() };
        assert_eq!(sum(&flat, &hits_flat, "rule 1"), sum(&set, &hits_set, "rule 1"), "the rule's counter adds up the same");
        assert_eq!(sum(&flat, &hits_flat, "rule 1"), 40 + 3, "forty IPv4 and three IPv6 sources on port 22");
    }

    #[test]
    fn below_the_threshold_nothing_changes() {
        let mut c = Config::default();
        c.presets.clear();
        let mut r = Resolved::default();
        let mut h = rule(Kind::Host, "small.example.com");
        h.proto = Proto::Tcp;
        h.ports = "22".into();
        c.add(h);
        r.addrs.insert("small.example.com".into(), many((SET_MIN - 1) as u8));
        let a = build(&c, &r, &Ctx::default()).unwrap().ruleset;
        let b = build_with(&c, &r, &Ctx::default(), usize::MAX).unwrap().ruleset;
        assert_eq!(a, b);
        assert!(a.sets.is_empty());
    }

    #[test]
    fn a_list_too_long_for_rules_fits_as_a_set() {
        let mut c = Config::default();
        c.presets.clear();
        let mut r = Resolved::default();
        c.add(rule(Kind::Host, "huge.example.com"));
        let addrs: Vec<IpAddr> = (0..3500u32).map(|i| IpAddr::V4(std::net::Ipv4Addr::from(0x0a64_0000 + i))).collect();
        r.addrs.insert("huge.example.com".into(), addrs);
        assert!(build_with(&c, &r, &Ctx::default(), usize::MAX).is_err(), "3500 rules are over the limit");
        let rs = build(&c, &r, &Ctx::default()).unwrap().ruleset;
        assert_eq!(rs.sets.len(), 1);
        assert_eq!(rs.sets[0].members.len(), 3500);
        assert!(eval(&rs, &Packet::new_tcp("eth0", ip("10.100.13.0"), 80)), "an address inside the list");
        assert!(!eval(&rs, &Packet::new_tcp("eth0", ip("10.100.13.200"), 80)), "just outside the list (3500 = 0xdac)");
    }

    fn saddr_rule(a: Addr) -> crate::ruleset::RuleSpec {
        crate::ruleset::RuleSpec { steps: vec![Step::Saddr(a), Step::L4(6), Step::Dport(22, 22), Step::Accept], note: "rule 1".into() }
    }

    fn prefix_rules() -> Vec<crate::ruleset::RuleSpec> {
        let mut v = Vec::new();
        for i in 0..20u8 {
            v.push(saddr_rule(Addr::Cidr(ip(&format!("10.{i}.0.0")), 16)));
        }
        v.push(saddr_rule(Addr::Range(ip("172.16.0.5"), ip("172.16.0.9"))));
        v.push(saddr_rule(Addr::Range(ip("172.16.0.8"), ip("172.16.0.20")))); // overlaps the last one
        v.push(saddr_rule(Addr::Exact(ip("192.168.1.1"))));
        v
    }

    #[test]
    fn prefixes_and_ranges_become_one_merged_interval_set_with_the_same_verdicts() {
        let rules = prefix_rules();
        let (collapsed, sets) = crate::ruleset::collapse_long_lists(rules.clone(), SET_MIN);
        assert_eq!(collapsed.len(), 1);
        assert_eq!(sets.len(), 1);
        assert!(sets[0].members.is_empty());
        // 20 adjacent /16s from 10.0 to 10.19 merge into one span; the two overlapping ranges merge; the single address stays
        assert_eq!(sets[0].ranges, vec![(ip("10.0.0.0"), ip("10.19.255.255")), (ip("172.16.0.5"), ip("172.16.0.20")), (ip("192.168.1.1"), ip("192.168.1.1"))]);
        let flat = Ruleset { rules, sets: vec![] };
        let set = Ruleset { rules: collapsed, sets };
        for src in ["10.0.0.0", "10.19.255.255", "10.20.0.0", "9.255.255.255", "172.16.0.4", "172.16.0.5", "172.16.0.20", "172.16.0.21", "192.168.1.1", "192.168.1.2", "2001:db8::1"] {
            let p = Packet::new_tcp("eth0", ip(src), 22);
            assert_eq!(eval(&flat, &p), eval(&set, &p), "{src}");
        }
        let t = set.to_text();
        assert!(t.contains("flags interval") && t.contains("10.0.0.0-10.19.255.255") && t.contains("192.168.1.1,") == false, "{t}");
    }

    fn port_rule(a: Addr, lo: u16, hi: u16) -> crate::ruleset::RuleSpec {
        crate::ruleset::RuleSpec { steps: vec![Step::Nfproto(2), Step::Saddr(a), Step::L4(6), Step::Dport(lo, hi), Step::Counter, Step::Accept], note: "rule 1".into() }
    }

    fn pair_rules() -> Vec<crate::ruleset::RuleSpec> {
        let mut v: Vec<_> = (0..16u8).map(|i| port_rule(Addr::Cidr(ip(&format!("10.{i}.0.0")), 16), 80 + u16::from(i), 80 + u16::from(i))).collect();
        v.push(port_rule(Addr::Range(ip("172.16.0.5"), ip("172.16.0.20")), 8000, 8100));
        v.push(port_rule(Addr::Exact(ip("192.168.1.5")), 443, 443));
        v
    }

    #[test]
    fn address_and_port_pairs_become_one_rule_and_one_set_with_the_same_verdicts_only_when_asked() {
        let rules = pair_rules();
        let mut sets = Vec::new();
        let collapsed = crate::ruleset::collapse_concat(rules.clone(), SET_MIN, &mut sets);
        assert_eq!((collapsed.len(), sets.len()), (1, 1));
        assert_eq!(collapsed[0].steps, vec![Step::Nfproto(2), Step::L4(6), Step::SaddrDportSet("c1".into()), Step::Counter, Step::Accept]);
        assert_eq!(sets[0].concat.len(), 18);
        let flat = Ruleset { rules, sets: vec![] };
        let set = Ruleset { rules: collapsed, sets };
        for src in ["10.0.0.0", "10.3.9.9", "10.15.255.255", "10.16.0.0", "172.16.0.4", "172.16.0.5", "172.16.0.20", "172.16.0.21", "192.168.1.5", "192.168.1.6", "2001:db8::1"] {
            for port in [22, 79, 80, 83, 95, 443, 444, 8000, 8050, 8100, 8101] {
                let p = Packet::new_tcp("eth0", ip(src), port);
                assert_eq!(eval(&flat, &p), eval(&set, &p), "{src}:{port}");
            }
        }
        let mut udp = Packet::new_tcp("eth0", ip("192.168.1.5"), 443);
        udp.l4 = 1;
        assert!(!eval(&set, &udp));
        // not asked: the plain builder never makes one; asked, a host with many answers and two ports becomes one rule and one pair set
        let mut c = Config::default();
        c.presets.clear();
        let mut h = Rule::new(Kind::Host, "big.example.com");
        h.proto = Proto::Tcp;
        h.ports = "80,443".into();
        c.add(h);
        let mut r = Resolved::default();
        r.addrs.insert("big.example.com".into(), (1..=20u8).map(|i| format!("198.51.100.{i}").parse().unwrap()).collect());
        let plain = build(&c, &r, &Ctx::default()).unwrap().ruleset;
        assert!(plain.sets.iter().all(|s| s.concat.is_empty()) && !plain.rules.iter().any(|r| r.steps.iter().any(|s| matches!(s, Step::SaddrDportSet(_)))));
        assert!(!crate::ruleset::BuildOptions::default().concat_sets);
        let on = crate::ruleset::build_opts(&c, &r, &Ctx::default(), crate::ruleset::BuildOptions { concat_sets: true, ..Default::default() }).unwrap().ruleset;
        assert_eq!(on.sets.iter().filter(|s| !s.concat.is_empty()).map(|s| s.concat.len()).collect::<Vec<_>>(), vec![40], "{}", on.to_text());
        assert!(on.rules.len() < plain.rules.len());
        for src in ["198.51.100.1", "198.51.100.20", "198.51.100.21", "9.9.9.9"] {
            for port in [79, 80, 81, 442, 443, 22] {
                let p = Packet::new_tcp("eth0", ip(src), port);
                assert_eq!(eval(&plain, &p), eval(&on, &p), "{src}:{port}");
            }
        }
    }

    /// Writes the batch that the pair-set ruleset makes to the file named by `IYAKKI_FW_DUMP` (a tool for loading it into a real kernel
    /// with a few lines of script in a scratch network namespace; see the F-FW-14 notes). Ignored: it needs the variable.
    #[test]
    #[ignore = "writes a netlink batch to the path in IYAKKI_FW_DUMP"]
    fn dump_the_pair_set_batch_for_a_kernel_run() {
        let Ok(path) = std::env::var("IYAKKI_FW_DUMP") else { return };
        let mut c = Config::default();
        c.presets.clear();
        let mut h = Rule::new(Kind::Host, "big.example.com");
        h.proto = Proto::Tcp;
        h.ports = "80,443".into();
        c.add(h);
        let mut r = Resolved::default();
        r.addrs.insert("big.example.com".into(), (1..=20u8).map(|i| format!("198.51.100.{i}").parse().unwrap()).collect());
        let rs = crate::ruleset::build_opts(&c, &r, &Ctx::default(), crate::ruleset::BuildOptions { concat_sets: true, ..Default::default() }).unwrap().ruleset;
        assert!(rs.sets.iter().any(|s| !s.concat.is_empty()));
        let (bytes, acks) = nl::encode_replace(&rs, 1);
        std::fs::write(&path, &bytes).unwrap();
        std::fs::write(format!("{path}.acks"), acks.to_string()).unwrap();
    }

    #[test]
    fn pairs_that_overlap_are_left_as_separate_rules() {
        let mut rules = pair_rules();
        rules.push(port_rule(Addr::Cidr(ip("10.0.0.0"), 8), 80, 90)); // intersects the first elements in both
        let mut sets = Vec::new();
        let out = crate::ruleset::collapse_concat(rules.clone(), SET_MIN, &mut sets);
        assert_eq!((out.len(), sets.len()), (rules.len(), 0));
        // fewer than the minimum stays as it is too
        let few = crate::ruleset::collapse_concat(pair_rules()[..5].to_vec(), SET_MIN, &mut sets);
        assert_eq!(few.len(), 5);
    }

    #[test]
    fn the_address_and_port_set_is_written_byte_for_byte_as_nft_writes_it() {
        // the bytes below are what nft 1.1.6 sent to a 7.0 kernel for
        //   set c1 { type ipv4_addr . inet_service; flags interval; elements = { 10.0.0.0/8 . 80-90, 192.168.1.5 . 443 } }
        //   ip saddr . th dport @c1 accept
        let set = crate::ruleset::AddrSet { name: "c1".into(), v4: true, members: vec![], ranges: vec![], concat: vec![((ip("10.0.0.0"), ip("10.255.255.255")), (80, 90)), ((ip("192.168.1.5"), ip("192.168.1.5")), (443, 443))] };
        let rs = Ruleset { rules: vec![crate::ruleset::RuleSpec { steps: vec![Step::Nfproto(2), Step::SaddrDportSet("c1".into()), Step::Accept], note: String::new() }], sets: vec![set.clone()] };
        let (bytes, _) = nl::encode_replace(&rs, 1);
        let m = nl::split_messages(&bytes);
        let newset = m.iter().find(|(t, _)| t & 0xff == 9).expect("a set is made").1;
        let has = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).any(|w| w == needle);
        assert!(has(newset, &[8, 0, 3, 0, 0, 0, 0, 0x84]), "flags interval + concatenation");
        assert!(has(newset, &[8, 0, 4, 0, 0, 0, 1, 0xcd]), "key type (ipaddr << 6) | service");
        assert!(has(newset, &[8, 0, 5, 0, 0, 0, 0, 8]), "padded key length");
        assert!(has(newset, &[8, 0, 10, 0, 0, 0, 0, 1]), "set id");
        assert!(has(newset, &[0x1c, 0, 2, 0x80, 0x0c, 0, 1, 0x80, 8, 0, 1, 0, 0, 0, 0, 4, 0x0c, 0, 1, 0x80, 8, 0, 1, 0, 0, 0, 0, 2]), "field lengths 4 and 2");
        let elems = m.iter().find(|(t, _)| t & 0xff == 12).expect("elements").1;
        let want: [u8; 72] = [
            0x24, 0, 1, 0x80, 0x10, 0, 1, 0x80, 0x0c, 0, 1, 0, 0x0a, 0, 0, 0, 0, 0x50, 0, 0, 0x10, 0, 0x0a, 0x80, 0x0c, 0, 1, 0, 0x0a, 0xff, 0xff, 0xff, 0, 0x5a, 0, 0, //
            0x24, 0, 1, 0x80, 0x10, 0, 1, 0x80, 0x0c, 0, 1, 0, 0xc0, 0xa8, 1, 5, 0x01, 0xbb, 0, 0, 0x10, 0, 0x0a, 0x80, 0x0c, 0, 1, 0, 0xc0, 0xa8, 1, 5, 0x01, 0xbb, 0, 0,
        ];
        assert!(has(elems, &[0x4c, 0, 3, 0x80]) && has(elems, &want), "elements: start key, then the last key as attribute 10");
        // the rule: address into register 1, port into register 9 (the next word), lookup of register 1
        let rule = m.iter().find(|(t, _)| t & 0xff == 6).expect("a rule").1;
        assert!(has(rule, &[8, 0, 1, 0, 0, 0, 0, 9, 8, 0, 2, 0, 0, 0, 0, 2, 8, 0, 3, 0, 0, 0, 0, 2, 8, 0, 4, 0, 0, 0, 0, 2]), "port: 2 bytes at the transport header + 2 into register 9");
        assert!(has(rule, b"lookup\0"));
        // the same set in nft's own words
        assert_eq!(set.concat_text(), "  set c1 {\n    type ipv4_addr . inet_service\n    flags interval\n    elements = { 10.0.0.0-10.255.255.255 . 80-90, 192.168.1.5 . 443 }\n  }\n");
    }

    #[test]
    fn an_interval_set_is_written_with_the_interval_flag_and_end_elements() {
        let (_, sets) = crate::ruleset::collapse_long_lists(prefix_rules(), SET_MIN);
        let mut rs = Ruleset { rules: vec![], sets };
        rs.rules.push(crate::ruleset::RuleSpec { steps: vec![Step::SaddrSet("l1".into()), Step::Accept], note: String::new() });
        let (bytes, _) = nl::encode_replace(&rs, 1);
        let m = nl::split_messages(&bytes);
        let newset = m.iter().find(|(t, _)| t & 0xff == 9).expect("a set is made").1.to_vec();
        // attribute 3 (flags) is 4 = NFT_SET_INTERVAL
        assert!(newset.windows(8).any(|w| w == [8, 0, 3, 0, 0, 0, 0, 4]), "{newset:?}");
        let elems: Vec<&[u8]> = m.iter().filter(|(t, _)| t & 0xff == 12).map(|(_, p)| *p).collect();
        assert_eq!(elems.len(), 1);
        // an end element for the first span is the address after it, 10.20.0.0, with flag 1
        assert!(elems[0].windows(4).any(|w| w == [10, 20, 0, 0]));
        assert!(elems[0].windows(8).any(|w| w == [8, 0, 3, 0, 0, 0, 0, 1]), "interval end flag");
        // the set is filled before the chain, as for plain sets
        let kinds: Vec<u16> = m.iter().map(|(t, _)| t & 0xff).collect();
        assert!(kinds.iter().position(|k| *k == 12) < kinds.iter().position(|k| *k == 3));
    }

    #[test]
    fn a_span_to_the_last_address_closes_with_an_all_zero_end_key_and_any_stays_a_rule() {
        let mut rules: Vec<_> = (0..16u8).map(|i| saddr_rule(Addr::Cidr(ip(&format!("{}.0.0.0", 240 + i)), 8))).collect();
        rules.push(saddr_rule(Addr::Cidr(ip("0.0.0.0"), 0)));
        let (collapsed, sets) = crate::ruleset::collapse_long_lists(rules, SET_MIN);
        assert_eq!(collapsed.len(), 2, "the any-address rule is kept as it is");
        assert_eq!(sets[0].ranges, vec![(ip("240.0.0.0"), ip("255.255.255.255"))]);
        let rs = Ruleset { rules: collapsed, sets };
        let (bytes, _) = nl::encode_replace(&rs, 1);
        let m = nl::split_messages(&bytes);
        let e = m.iter().find(|(t, _)| t & 0xff == 12).unwrap();
        assert!(e.1.windows(8).any(|w| w == [8, 0, 3, 0, 0, 0, 0, 1]));
    }

    #[test]
    fn the_batch_makes_the_set_and_fills_it_before_the_chain_and_the_rule_looks_it_up() {
        let (c, r) = host_config();
        let rs = build(&c, &r, &Ctx::default()).unwrap().ruleset;
        let (bytes, acks) = nl::encode_replace(&rs, 1);
        let m = nl::split_messages(&bytes);
        let kinds: Vec<u16> = m.iter().map(|(t, _)| t & 0xff).collect();
        let newset = kinds.iter().position(|k| *k == 9).expect("a set is made");
        let elems = kinds.iter().position(|k| *k == 12).expect("and filled");
        let chain = kinds.iter().position(|k| *k == 3).unwrap();
        let first_rule = kinds.iter().position(|k| *k == 6).unwrap();
        assert!(newset < elems && elems < chain && chain < first_rule);
        let payload = &m[elems].1;
        let list = nl::attrs(&payload[4..]).into_iter().find(|(t, _)| *t == 3).unwrap().1;
        assert_eq!(nl::attrs(list).len(), 40, "forty elements");
        let n_rules = m.iter().filter(|(t, _)| t & 0xff == 6).count();
        assert_eq!(acks, 3 + 2 + 1 + n_rules);
        let with_lookup = m.iter().filter(|(t, _)| t & 0xff == 6).filter(|(_, p)| expr_names(p).iter().any(|n| n == "lookup")).count();
        assert_eq!(with_lookup, 1, "one rule looks the source up");
    }

    #[test]
    fn the_engine_hands_out_the_counts_of_the_installed_rules_only() {
        let mut r = rig();
        assert!(r.e.rule_counts().is_empty(), "nothing before the firewall is in force");
        r.e.boot(false);
        let mut c = r.e.config().clone();
        c.add(rule(Kind::Ip, "198.51.100.7"));
        r.e.propose(c, "admin").unwrap();
        let installed = r.be.installed().unwrap();
        let per: Vec<Option<(u64, u64)>> = installed.rules.iter().map(|x| x.steps.contains(&Step::Counter).then_some((5, 300))).collect();
        r.be.0.lock().unwrap().rule_counters = per.clone();
        let counts = r.e.rule_counts();
        let one = counts.iter().find(|c| c.note == "rule 1").expect("the rule of the page");
        assert_eq!((one.packets, one.bytes), (5, 300));
        assert!(counts.iter().all(|c| c.note != BLOCKED_NOTE));
        r.be.0.lock().unwrap().rule_counters = per[1..].to_vec();
        assert!(r.e.rule_counts().is_empty(), "a list for another table gives nothing");
    }
}

// ------------------------------------------------------------------ outgoing rules in the engine (F-FW-10)

fn out_cfg(block: bool) -> crate::outbound::OutConfig {
    use crate::outbound::{OutConfig, OutRule, Verdict};
    OutConfig {
        enabled: true,
        block_by_default: block,
        rules: vec![OutRule { id: 1, enabled: true, verdict: Verdict::Allow, dest: String::new(), proto: Proto::Tcp, ports: "443".into(), iface: String::new(), note: String::new() }],
        forwards: Vec::new(),
        port_forwards: Vec::new(),
    }
}

#[test]
fn fw_out_boot_puts_the_saved_outgoing_tables_in_and_a_change_waits_for_keep() {
    let store = MemStore::default();
    let mut c = Config::default();
    c.outgoing = out_cfg(false);
    *store.saved.lock().unwrap() = Some(Saved { config: c, resolved: Resolved::default() });
    let mut r = rig_with(store);
    r.e.boot(false);
    assert_eq!(r.be.0.lock().unwrap().out_tables, vec!["iyakki_fw_out".to_string()], "boot loads the outgoing table");
    // blocking by default is a tightening: 60 s Keep/Undo
    let mut n = r.e.config().clone();
    n.outgoing = out_cfg(true);
    r.e.propose(n, "admin").unwrap();
    assert!(matches!(r.e.status(), Status::Pending { .. }));
    r.advance(61);
    r.e.tick();
    assert!(!r.e.config().outgoing.block_by_default, "not kept: the old outgoing rules are back");
    assert_eq!(r.be.0.lock().unwrap().out_tables.len(), 1);
}

#[test]
fn fw_out_turning_the_firewall_or_the_outgoing_part_off_takes_the_tables_out() {
    let mut r = rig();
    r.e.boot(false);
    let mut n = r.e.config().clone();
    n.outgoing = out_cfg(false);
    r.e.propose(n.clone(), "admin").unwrap();
    r.e.keep("admin").unwrap();
    assert_eq!(r.be.0.lock().unwrap().out_tables.len(), 1);
    n.outgoing.enabled = false;
    r.e.propose(n, "admin").unwrap();
    assert!(r.be.0.lock().unwrap().out_tables.is_empty());
    r.e.keep("admin").unwrap();
    let mut n = r.e.config().clone();
    n.outgoing.enabled = true;
    r.e.propose(n, "admin").unwrap();
    r.e.keep("admin").unwrap();
    r.e.disable_now("admin").unwrap();
    assert!(r.be.0.lock().unwrap().out_tables.is_empty(), "the emergency switch removes the outgoing tables too");
}

#[test]
fn fw_out_a_failing_outgoing_step_puts_the_old_settings_back_and_bad_rules_are_refused() {
    let mut r = rig();
    r.e.boot(false);
    r.be.0.lock().unwrap().fail_out = Some(BackendError::Failed("no nft_reject".into()));
    let mut n = r.e.config().clone();
    n.add(rule(Kind::Ip, "198.51.100.7"));
    n.outgoing = out_cfg(false);
    let err = r.e.propose(n, "admin").unwrap_err();
    assert!(err.contains("outgoing rules could not be applied"), "{err}");
    assert!(!r.be.installed().unwrap().to_text().contains("198.51.100.7"), "the input table is back to the old rules");
    r.be.0.lock().unwrap().fail_out = None;
    let mut bad = r.e.config().clone();
    bad.outgoing = out_cfg(true);
    bad.outgoing.rules.clear();
    assert!(r.e.propose(bad, "admin").is_err(), "blocking by default with no allow rule is refused");
}

#[test]
fn fw_out_an_old_settings_file_without_the_outgoing_part_still_loads() {
    let c = Config::default();
    let mut v = serde_json::to_value(&c).unwrap();
    v.as_object_mut().unwrap().remove("outgoing");
    let back: Config = serde_json::from_value(v).unwrap();
    assert!(!back.outgoing.enabled);
}
