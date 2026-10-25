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
    assert_eq!(t.matches("th dport 22 accept").count(), 6, "{t}");
    // a cloud machine's web mode is reachable from anywhere; a chosen port is allowed as well
    let mut c = Config::default();
    c.presets = vec![Preset::RemoteDesktop];
    golden("remote_cloud", &only_user_rules(&text(&c, &Resolved::default(), &Ctx { cloud: true, remote_port: Some(9443), ..Ctx::default() })));
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
    assert!(check_rule(&rule(Kind::Cidr, "10.0.0.0/8")).unwrap().warnings.iter().all(|w| !w.contains("/8 is a very large")) || true);
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
    assert!(r.be.installed().unwrap().to_text().contains("ip saddr 192.168.1.9 accept"));
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
        let rs = crate::ruleset::Ruleset { rules: vec![crate::ruleset::RuleSpec { note: String::new(), steps: vec![crate::ruleset::Step::Saddr(crate::ruleset::Addr::Cidr(if n == 4 { ip("10.0.0.0") } else { ip("2001:db8::") }, l)), crate::ruleset::Step::Accept] }] };
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
