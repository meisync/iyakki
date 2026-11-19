//! The VPN manager against the fake system layer: import, secrets in the vault only, a FULL connect and its exact undo, the
//! kill switch and the fail-closed guard (installed first, kept after a drop, removed on disable), HYBRID routes and DNS-driven
//! routes, refusals, recovery after a crash. Every key below is FAKE (32 bytes of one repeated value); nothing touches a network.

use std::sync::{Arc, Mutex};

use iyakki_vpn::dnsroute::{Action, Event, Upstream};
use iyakki_vpn::dnswire::{build_query, build_response, with_id, TYPE_A};
use iyakki_vpn::filter::FilterSet;
use iyakki_vpn::profile::Settings;
use iyakki_vpn::{b64, Mode};

use crate::gitkeys::{AuditFn, MemStore, SecretStore};
use crate::vpn_mgr::{valid_id, Manager, State, IFNAME};
use crate::vpn_sys::{parse_addrs, parse_default_route, FakeSys, Sys};

const NOW: u64 = 1_000_000;

pub(crate) fn key(n: u8) -> String {
    b64::encode(&[n; 32])
}

pub(crate) fn conf(extra_allowed: &str, endpoint: &str) -> String {
    format!(
        "[Interface]\nPrivateKey = {}\nAddress = 10.8.0.2/24\nDNS = 10.8.0.1\nMTU = 1380\n\n[Peer]\nPublicKey = {}\nPresharedKey = {}\nEndpoint = {endpoint}\nAllowedIPs = {extra_allowed}\nPersistentKeepalive = 25\n",
        key(0x11),
        key(0x22),
        key(0x33)
    )
}

pub(crate) struct Rig {
    pub mgr: Arc<Manager>,
    pub sys: Arc<FakeSys>,
    pub vault: Arc<MemStore>,
    pub log: Arc<Mutex<Vec<String>>>,
    pub dir: std::path::PathBuf,
}

pub(crate) fn temp(tag: &str) -> std::path::PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 4];
    iyakki_sec::random_fill(&mut r).unwrap();
    let d = std::path::Path::new(&base).join(format!("vpn-{tag}-{:02x}{:02x}{:02x}{:02x}", r[0], r[1], r[2], r[3]));
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(crate) fn rig(tag: &str) -> Rig {
    let dir = temp(tag);
    let sys = Arc::new(FakeSys::new());
    sys.name("vpn.example.com", &["203.0.113.9"]);
    let vault = Arc::new(MemStore::default());
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    let audit: AuditFn = Arc::new(move |a, b| l2.lock().unwrap().push(format!("{a} {b}")));
    let mgr = Arc::new(Manager::new(&dir, vault.clone(), sys.clone(), audit));
    Rig { mgr, sys, vault, log, dir }
}

pub(crate) fn import_full(r: &Rig, name: &str) -> String {
    r.mgr.import(name, conf("0.0.0.0/0, ::/0", "vpn.example.com:51820").as_bytes()).unwrap().profile.id
}

fn hs(r: &Rig, at: i64) {
    r.sys.st.lock().unwrap().handshake = Some(at);
}

fn calls_index(r: &Rig, needle: &str) -> usize {
    r.sys.calls().iter().position(|c| c.contains(needle)).unwrap_or_else(|| panic!("no call with {needle}: {:?}", r.sys.calls()))
}

fn full_settings(kill: bool) -> Settings {
    Settings { kill_switch: kill, ..Settings::full() }
}

#[test]
fn import_keeps_the_keys_in_the_vault_and_nowhere_else() {
    let r = rig("import");
    let id = import_full(&r, "Office");
    assert!(valid_id(&id));
    let names = r.vault.names().unwrap();
    assert!(names.contains(&format!("vpn.{id}.key")) && names.contains(&format!("vpn.{id}.psk.0")));
    // the profile file, the audit log, the rows and the status hold no key
    let file = std::fs::read_to_string(r.dir.join("profiles.json")).unwrap();
    let everything = format!("{file}{:?}{:?}{:?}", r.log.lock().unwrap(), r.mgr.rows(), r.mgr.profiles());
    for k in [key(0x11), key(0x33)] {
        assert!(!everything.contains(&k), "a key leaked");
    }
    assert!(file.contains(&key(0x22)), "the peer's PUBLIC key is kept");
    let rows = r.mgr.rows();
    assert_eq!((rows.len(), rows[0].enabled), (1, false));
    assert_eq!(r.mgr.status(&id).state, State::Stopped);
    assert!(r.log.lock().unwrap().iter().any(|l| l.starts_with("vpn.import Office")));
    // the file is private
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(r.dir.join("profiles.json")).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn a_refused_file_leaves_nothing_behind_and_names_no_value() {
    let r = rig("refused");
    let bad = conf("0.0.0.0/0", "vpn.example.com:51820").replace("[Peer]", "PostUp = /bin/sh -c evil\n[Peer]");
    let e = r.mgr.import("Bad", bad.as_bytes()).unwrap_err();
    assert!(e.contains("PostUp") && !e.contains("evil") && !e.contains(&key(0x11)), "{e}");
    assert!(r.vault.names().unwrap().is_empty());
    assert!(r.mgr.profiles().is_empty());
    assert!(!r.dir.join("profiles.json").exists());
    import_full(&r, "Office");
    assert!(r.mgr.import("office", conf("0.0.0.0/0", "vpn.example.com:51820").as_bytes()).unwrap_err().contains("already"));
    assert!(r.mgr.import("x", b"").is_err());
}

#[test]
fn the_vault_failing_during_import_rolls_back() {
    let r = rig("vaultfail");
    *r.vault.locked.lock().unwrap() = true;
    assert!(r.mgr.import("Office", conf("0.0.0.0/0", "vpn.example.com:51820").as_bytes()).is_err());
    assert!(r.mgr.profiles().is_empty());
}

#[test]
fn the_firewall_is_told_the_tunnels_network_while_it_is_up_and_nothing_when_it_is_down() {
    let r = rig("fwnets");
    let id = import_full(&r, "Office");
    assert!(r.sys.st.lock().unwrap().tunnel_nets.is_empty());
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    assert_eq!(r.sys.st.lock().unwrap().tunnel_nets, vec!["10.8.0.0/24".to_string()], "the network, not the host address");
    r.mgr.set_enabled(&id, false, NOW + 3).unwrap();
    assert!(r.sys.st.lock().unwrap().tunnel_nets.is_empty());
}

#[test]
fn a_full_connect_sets_up_in_order_and_disable_undoes_all_of_it() {
    let r = rig("full");
    let id = import_full(&r, "Office");
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    {
        let s = r.sys.st.lock().unwrap();
        assert!(s.links.contains(IFNAME));
        // endpoint pin through the old gateway, then the two halves through the tunnel; the default route is untouched
        assert!(s.routes.contains(&format!("203.0.113.9/32 via 192.168.1.1 dev eth0")), "{:?}", s.routes);
        assert!(s.routes.contains(&format!("0.0.0.0/1 dev {IFNAME}")) && s.routes.contains(&format!("128.0.0.0/1 dev {IFNAME}")));
        assert!(!s.routes.iter().any(|x| x.starts_with("default") || x.starts_with("0.0.0.0/0 ")));
        // DNS is the VPN's, the original is saved
        assert!(s.resolv.as_deref().unwrap().contains("nameserver 10.8.0.1") && s.resolv.as_deref().unwrap().contains("IYAKKI-VPN-DNS"));
        assert_eq!(s.backup.as_deref(), Some("nameserver 192.168.1.1\n"));
        // the kernel got the real private key, the log lines carry none
        assert_eq!(s.last_private.as_deref(), Some(&[0x11u8; 32][..]));
        assert!(s.guard.is_none(), "kill switch is off by default");
        assert!(!s.calls.join("|").contains(&key(0x11)));
    }
    assert!(calls_index(&r, "link add") < calls_index(&r, "wg configure") && calls_index(&r, "wg configure") < calls_index(&r, "address add"));
    assert!(calls_index(&r, "address add") < calls_index(&r, "link set") && calls_index(&r, "link set") < calls_index(&r, "route add"));
    assert_eq!(r.mgr.status(&id).state, State::Connecting, "up, but no handshake yet");
    // a handshake makes it connected; an old one does not
    hs(&r, NOW as i64 - 1000);
    r.mgr.tick(NOW + 1);
    assert_eq!(r.mgr.status(&id).state, State::Connecting);
    hs(&r, NOW as i64);
    r.mgr.tick(NOW + 2);
    assert_eq!(r.mgr.status(&id).state, State::Connected { since: NOW + 2 });
    assert!(r.mgr.status(&id).line(true, NOW + 62).starts_with("connected, 1 min"));
    // disable removes every trace and gives the original DNS back
    r.mgr.set_enabled(&id, false, NOW + 3).unwrap();
    assert!(r.sys.clean(), "{:?}", r.sys.st.lock().unwrap().routes);
    assert_eq!(r.sys.read_resolv().as_deref(), Some("nameserver 192.168.1.1\n"));
    assert_eq!(r.mgr.status(&id).state, State::Stopped);
    assert!(!r.mgr.is_enabled(&id));
}

#[test]
fn deleting_a_connected_profile_disconnects_and_empties_the_vault() {
    let r = rig("delete");
    let id = import_full(&r, "Office");
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    let done = r.mgr.remove(&id).unwrap();
    assert!(done[0].contains("Office") && done.iter().any(|d| d.contains("2 secrets")), "{done:?}");
    assert!(r.sys.clean());
    assert!(r.vault.names().unwrap().is_empty());
    assert!(r.mgr.profiles().is_empty());
}

#[test]
fn the_kill_switch_guard_goes_in_first_stays_after_a_drop_and_goes_with_disable() {
    let r = rig("ks");
    let id = import_full(&r, "Office");
    r.mgr.set_settings(&id, full_settings(true), NOW).unwrap();
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    assert!(calls_index(&r, "guard install") < calls_index(&r, "link add"), "fail closed: guard before the tunnel");
    assert!(r.sys.guard_present() && r.mgr.status(&id).guard);
    let spec = r.sys.st.lock().unwrap().guard.clone().unwrap();
    assert!(spec.render().contains("everything else"), "FULL ends in a drop");
    hs(&r, NOW as i64);
    r.mgr.tick(NOW + 1);
    assert!(matches!(r.mgr.status(&id).state, State::Connected { .. }));
    // the interface is gone (module unloaded, link removed): the tunnel parts go, the guard stays
    r.sys.st.lock().unwrap().links.clear();
    r.mgr.tick(NOW + 2);
    {
        let s = r.sys.st.lock().unwrap();
        assert!(s.guard.is_some(), "the guard must outlive the tunnel");
        assert!(s.routes.is_empty() && s.backup.is_none());
        assert!(!s.resolv.as_deref().unwrap().contains("IYAKKI"), "DNS restored");
    }
    let st = r.mgr.status(&id);
    assert!(st.error.is_some() && st.retry_at.is_some() && st.guard);
    // it comes back by itself after the back-off
    r.mgr.tick(NOW + 3);
    assert_eq!(r.mgr.status(&id).state, State::Stopped, "not yet: the back-off has not passed");
    r.mgr.tick(NOW + 2 + 60);
    assert_eq!(r.mgr.status(&id).state, State::Connecting);
    r.mgr.set_enabled(&id, false, NOW + 100).unwrap();
    assert!(r.sys.clean(), "disable removes the guard too");
}

#[test]
fn if_the_guard_cannot_be_installed_nothing_connects() {
    let r = rig("guardfail");
    let id = import_full(&r, "Office");
    r.mgr.set_settings(&id, full_settings(true), NOW).unwrap();
    r.sys.fail("guard install");
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    let st = r.mgr.status(&id);
    assert!(st.needs_you && st.error.as_deref().unwrap().contains("guard"), "{st:?}");
    assert!(r.sys.clean(), "no interface, no routes");
    assert!(!r.sys.calls().iter().any(|c| c.contains("link add")));
}

#[test]
fn a_failure_midway_undoes_the_steps_before_it() {
    let r = rig("midway");
    let id = import_full(&r, "Office");
    r.sys.fail("route add");
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    let st = r.mgr.status(&id);
    assert!(st.error.is_some() && !st.needs_you && st.retry_at.is_some(), "{st:?}");
    assert!(r.sys.clean(), "link, routes and DNS rolled back: {:?}", r.sys.st.lock().unwrap().routes);
    // with a kill switch the same failure keeps the guard and says so
    r.mgr.set_enabled(&id, false, NOW).unwrap();
    r.mgr.set_settings(&id, full_settings(true), NOW).unwrap();
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    assert!(r.sys.guard_present() && r.mgr.status(&id).error.unwrap().contains("guard stays"));
    // the problem goes away, the retry works
    r.sys.clear_failures();
    r.mgr.tick(NOW + 1000);
    assert_eq!(r.mgr.status(&id).state, State::Connecting);
    r.mgr.set_enabled(&id, false, NOW + 1001).unwrap();
    assert!(r.sys.clean());
}

#[test]
fn the_server_is_resolved_once_checked_and_pinned() {
    let r = rig("endpoint");
    // a server on the cluster network is refused, with the reason, and nothing is touched
    r.sys.name("evil.example.com", &["10.43.0.5"]);
    let id = r.mgr.import("Evil", conf("0.0.0.0/0", "evil.example.com:51820").as_bytes()).unwrap().profile.id;
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    let st = r.mgr.status(&id);
    assert!(st.needs_you && st.error.unwrap().contains("cluster"), "refused for the cluster address");
    assert!(r.sys.clean());
    // a name that does not resolve is a soft failure that is retried
    r.sys.name("gone.example.com", &[]);
    r.mgr.set_enabled(&id, false, NOW).unwrap();
    let id2 = r.mgr.import("Gone", conf("0.0.0.0/0", "nowhere.example.com:51820").as_bytes()).unwrap().profile.id;
    r.mgr.set_enabled(&id2, true, NOW).unwrap();
    let st = r.mgr.status(&id2);
    assert!(!st.needs_you && st.retry_at.is_some());
    // a server on the local network connects, with a note
    let id3 = r.mgr.import("Lan", conf("0.0.0.0/0", "192.168.1.50:51820").as_bytes()).unwrap().profile.id;
    r.mgr.set_enabled(&id3, true, NOW).unwrap();
    assert!(r.mgr.status(&id3).warnings.iter().any(|w| w.contains("local network")));
    assert!(!r.mgr.is_enabled(&id2), "enabling one disables the other");
}

#[test]
fn only_one_profile_is_enabled_at_a_time() {
    let r = rig("one");
    let a = import_full(&r, "A");
    let b = import_full(&r, "B");
    r.mgr.set_enabled(&a, true, NOW).unwrap();
    r.mgr.set_enabled(&b, true, NOW + 1).unwrap();
    assert!(!r.mgr.is_enabled(&a) && r.mgr.is_enabled(&b));
    assert_eq!(r.mgr.status(&a).state, State::Stopped);
    assert_eq!(r.sys.st.lock().unwrap().links.len(), 1);
    r.mgr.stop_all();
    assert!(r.sys.clean());
}

#[test]
fn settings_are_validated() {
    let r = rig("settings");
    let id = import_full(&r, "Office");
    // HYBRID needs an include rule; FULL takes no filters; a catch-all is refused when the filters are parsed
    assert!(r.mgr.set_settings(&id, Settings::hybrid(FilterSet::new()), NOW).is_err());
    assert!(FilterSet::parse("*.com").is_err());
    let mut s = Settings::full();
    s.filters = FilterSet::parse("example.org").unwrap();
    assert!(r.mgr.set_settings(&id, s, NOW).is_err());
    assert!(r.mgr.set_settings("v99", Settings::full(), NOW).is_err());
    let ok = r.mgr.set_settings(&id, Settings::hybrid(FilterSet::parse("*.corp.example.com\n203.0.113.0/24").unwrap()), NOW).unwrap();
    assert_eq!(ok.settings.mode, Mode::Hybrid);
    assert!(ok.settings.fail_closed && !ok.settings.kill_switch);
}

#[test]
fn hybrid_routes_only_the_matched_networks_and_learns_names_from_dns() {
    let r = rig("hybrid");
    let id = import_full(&r, "Office");
    r.mgr.set_settings(&id, Settings::hybrid(FilterSet::parse("*.corp.example.com\n198.51.100.0/24").unwrap()), NOW).unwrap();
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    {
        let s = r.sys.st.lock().unwrap();
        assert!(s.routes.contains(&format!("198.51.100.0/24 dev {IFNAME}")), "{:?}", s.routes);
        assert!(!s.routes.iter().any(|x| x.starts_with("0.0.0.0/1") || x.starts_with("128.0.0.0/1")), "no halves in HYBRID");
        assert!(s.resolv.as_deref().unwrap() == "nameserver 192.168.1.1\n", "the system DNS is untouched in HYBRID");
        assert!(s.guard.is_some() && s.capture.is_some(), "fail closed is on by default, and the DNS capture is in place");
    }
    assert!(calls_index(&r, "guard install") < calls_index(&r, "link add"));
    hs(&r, NOW as i64);
    r.mgr.tick(NOW + 1);
    // a program asks for a tunnel name: the forwarder asks the VPN's DNS, the answer becomes a host route and a guard entry
    let q = build_query(0x4242, "app.corp.example.com", TYPE_A);
    let out = r.mgr.dns_event(Event::Query { client: 7, packet: q, upstream_id: 0x9999, now: NOW + 2 }, NOW + 2);
    let Some(Action::Send { to: Upstream::Vpn, packet }) = out.first().cloned() else { panic!("{out:?}") };
    let reply = build_response(&packet, 0, &[("203.0.113.77".parse().unwrap(), 60)]).unwrap();
    let out = r.mgr.dns_event(Event::Reply { from: Upstream::Vpn, packet: with_id(&reply, 0x9999), now: NOW + 3 }, NOW + 3);
    assert!(out.iter().any(|a| matches!(a, Action::Respond { client: 7, .. })));
    {
        let s = r.sys.st.lock().unwrap();
        assert!(s.routes.contains(&format!("203.0.113.77/32 dev {IFNAME}")), "{:?}", s.routes);
        assert!(s.guard_members.contains("203.0.113.77"));
    }
    // a name nobody listed is refused by the forwarder from the tunnel's DNS (it goes to the system resolver instead)
    let q2 = build_query(1, "www.example.org", TYPE_A);
    let out = r.mgr.dns_event(Event::Query { client: 8, packet: q2, upstream_id: 5, now: NOW + 4 }, NOW + 4);
    assert!(matches!(out.first(), Some(Action::Send { to: Upstream::System, .. })));
    // disable removes the learned route, the set entry, the capture and the guard
    r.mgr.set_enabled(&id, false, NOW + 5).unwrap();
    assert!(r.sys.clean());
}

#[test]
fn hybrid_with_domain_rules_refuses_to_connect_when_the_capture_cannot_be_installed_and_fail_closed_is_on() {
    let r = rig("hybrid-cap");
    let id = import_full(&r, "Office");
    r.mgr.set_settings(&id, Settings::hybrid(FilterSet::parse("*.corp.example.com").unwrap()), NOW).unwrap();
    r.sys.fail("capture install");
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    let st = r.mgr.status(&id);
    assert!(st.error.as_deref().unwrap().contains("capture"), "{st:?}");
    assert!(r.sys.st.lock().unwrap().links.is_empty());
    // with fail closed off the same problem is a warning and the tunnel comes up
    r.mgr.set_enabled(&id, false, NOW).unwrap();
    let mut s = Settings::hybrid(FilterSet::parse("*.corp.example.com").unwrap());
    s.fail_closed = false;
    r.mgr.set_settings(&id, s, NOW).unwrap();
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    assert_eq!(r.mgr.status(&id).state, State::Connecting);
    assert!(r.mgr.status(&id).warnings.iter().any(|w| w.contains("domain rules will not work")));
    assert!(r.sys.st.lock().unwrap().guard.is_none());
}

#[test]
fn openvpn_profiles_are_kept_but_not_connected_yet() {
    let r = rig("ovpn");
    let text = "client\ndev tun\nproto udp\nremote vpn.example.com 1194\nnobind\nremote-cert-tls server\n<ca>\n-----BEGIN CERTIFICATE-----\nFAKECA\n-----END CERTIFICATE-----\n</ca>\n<key>\n-----BEGIN PRIVATE KEY-----\nFAKE-KEY-CANARY\n-----END PRIVATE KEY-----\n</key>\n";
    let id = r.mgr.import("Home", text.as_bytes()).unwrap().profile.id;
    assert!(r.vault.names().unwrap().contains(&format!("vpn.{id}.blk.key")));
    let file = std::fs::read_to_string(r.dir.join("profiles.json")).unwrap();
    assert!(!file.contains("FAKE-KEY-CANARY"));
    let e = r.mgr.set_enabled(&id, true, NOW).unwrap_err();
    assert!(e.contains("OpenVPN") && e.contains("download"), "{e}");
    assert!(!r.mgr.is_enabled(&id));
    assert!(r.sys.clean());
    assert!(r.mgr.remove(&id).unwrap().iter().any(|d| d.contains("secret")));
    assert!(r.vault.names().unwrap().is_empty());
}

#[test]
fn recover_removes_what_a_crash_left_and_keeps_a_wanted_guard() {
    let r = rig("recover");
    let id = import_full(&r, "Office");
    {
        let mut s = r.sys.st.lock().unwrap();
        s.links.insert(IFNAME.into());
        s.routes.insert(format!("0.0.0.0/1 dev {IFNAME}"));
        s.resolv = Some("# IYAKKI-VPN-DNS: written while a VPN is connected; the original is restored on disconnect\nnameserver 10.8.0.1\n".into());
        s.backup = Some("nameserver 192.168.1.1\n".into());
        s.capture = Some(iyakki_vpn::plan::dns_capture(15353).unwrap());
        s.guard = Some(iyakki_vpn::plan::kill_switch(&iyakki_vpn::plan::KillSwitchInput { base: iyakki_vpn::plan::GuardBase { ifname: IFNAME, physical_ifs: &["eth0".to_string()], endpoints: &[], lan: &[], cluster: &[], cluster_if_prefixes: &[], dev_port: 7979 }, block_dns_outside: false }).unwrap());
    }
    // nothing enabled: everything goes, the guard too
    r.mgr.recover();
    assert!(r.sys.st.lock().unwrap().links.is_empty() && r.sys.st.lock().unwrap().capture.is_none() && r.sys.st.lock().unwrap().guard.is_none());
    assert_eq!(r.sys.read_resolv().as_deref(), Some("nameserver 192.168.1.1\n"));
    // an enabled profile with a kill switch keeps the guard through a restart
    r.mgr.set_settings(&id, full_settings(true), NOW).unwrap();
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    r.mgr.stop_all();
    let spec = r.sys.st.lock().unwrap().guard.clone();
    assert!(spec.is_none(), "stop_all removed it; install again as a crashed process would leave it");
    r.sys.st.lock().unwrap().guard = Some(iyakki_vpn::plan::kill_switch(&iyakki_vpn::plan::KillSwitchInput { base: iyakki_vpn::plan::GuardBase { ifname: IFNAME, physical_ifs: &["eth0".to_string()], endpoints: &[], lan: &[], cluster: &[], cluster_if_prefixes: &[], dev_port: 7979 }, block_dns_outside: false }).unwrap());
    let fresh = Manager::new(&r.dir, r.vault.clone(), r.sys.clone(), Arc::new(|_, _| {}));
    fresh.recover();
    assert!(r.sys.guard_present(), "an enabled kill-switch profile keeps its guard across a restart");
}

#[test]
fn a_stalled_handshake_is_reported_and_a_stale_one_is_reconnecting() {
    let r = rig("stall");
    let id = import_full(&r, "Office");
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    r.mgr.tick(NOW + 60);
    assert!(r.mgr.status(&id).error.unwrap().contains("no answer"));
    hs(&r, (NOW + 60) as i64);
    r.mgr.tick(NOW + 61);
    assert!(matches!(r.mgr.status(&id).state, State::Connected { .. }) && r.mgr.status(&id).error.is_none());
    // the heartbeat keeps its normal pace (a gap longer than RESUME_GAP would be taken for a resume from sleep)
    for k in 1..=7 {
        r.mgr.tick(NOW + 61 + 60 * k);
        if r.mgr.status(&id).state == State::Connecting {
            break;
        }
    }
    let st = r.mgr.status(&id);
    assert_eq!(st.state, State::Connecting);
    assert!(st.error.unwrap().contains("old"));
    r.mgr.stop_all();
}

#[test]
fn no_network_is_a_soft_failure_and_a_hard_one_is_not_retried_by_the_heartbeat() {
    let r = rig("nonet");
    let id = import_full(&r, "Office");
    r.sys.st.lock().unwrap().no_network = true;
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    let st = r.mgr.status(&id);
    assert!(!st.needs_you && st.retry_at.is_some() && st.error.unwrap().contains("network"));
    r.sys.st.lock().unwrap().no_network = false;
    r.mgr.tick(NOW + 5000);
    assert_eq!(r.mgr.status(&id).state, State::Connecting);
    // a locked vault is soft too (the vault opens after sign-in)
    r.mgr.stop_all();
    *r.vault.locked.lock().unwrap() = true;
    r.mgr.retry(&id, NOW + 6000);
    let st = r.mgr.status(&id);
    assert!(!st.needs_you && st.error.unwrap().contains("vault"));
    assert!(r.sys.clean());
}

#[test]
fn parsers_for_the_ip_tool_output() {
    assert_eq!(parse_default_route("default via 192.168.1.1 dev eth0 proto dhcp metric 100\n"), Some(("eth0".into(), Some("192.168.1.1".into()))));
    assert_eq!(parse_default_route("default dev ppp0 scope link\n"), Some(("ppp0".into(), None)));
    assert_eq!(parse_default_route(""), None);
    let a = "2: eth0    inet 192.168.1.5/24 brd 192.168.1.255 scope global dynamic eth0\n3: cni0    inet 10.42.0.1/24 scope global cni0\n4: wlan0    inet6 2001:db8::5/64 scope global\n5: iyvpn0    inet 10.8.0.2/24 scope global iyvpn0\n";
    let v = parse_addrs(a, &crate::vpn_sys::NOT_LAN_PREFIXES);
    assert_eq!(v.iter().map(|(d, c)| format!("{d} {c}")).collect::<Vec<_>>(), vec!["eth0 192.168.1.0/24", "wlan0 2001:db8::/64"]);
}

#[test]
#[ignore = "needs a Linux kernel with generic netlink; asks for the wireguard family and reports honestly if the module is absent"]
fn the_real_netlink_socket_asks_for_the_wireguard_family() {
    let s = crate::vpn_sys::OsSys::default();
    let r = s.wg_status("no-such-if0");
    assert!(r.is_err());
    println!("{}", r.unwrap_err());
}

// ---- the forwarder on real loopback sockets (fake system layer) ----------------------------------------------------------------

fn free_port() -> u16 {
    crate::testdir::free_port()
}

fn rig_sockets(tag: &str, port: u16) -> Rig {
    let mut r = rig(tag);
    let audit: AuditFn = {
        let l = r.log.clone();
        Arc::new(move |a, b| l.lock().unwrap().push(format!("{a} {b}")))
    };
    let m = Manager::new(&r.dir, r.vault.clone(), r.sys.clone(), audit).with_dns_sockets(port, None);
    r.mgr = Arc::new(m);
    r
}

#[test]
fn a_hybrid_connect_listens_before_the_redirect_goes_in_and_disable_frees_the_port() {
    let port = free_port();
    let r = rig_sockets("hybrid-sock", port);
    let id = import_full(&r, "Office");
    r.mgr.set_settings(&id, Settings::hybrid(FilterSet::parse("*.corp.example.com").unwrap()), NOW).unwrap();
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    assert_eq!(r.mgr.status(&id).state, State::Connecting, "{:?}", r.mgr.status(&id));
    {
        let s = r.sys.st.lock().unwrap();
        assert_eq!(s.capture.as_ref().map(|c| c.listen_port), Some(port), "the redirect points at the forwarder's port");
        // the VPN's DNS server sits inside the tunnel's own network: no extra host route was needed
        assert!(!s.routes.iter().any(|x| x.starts_with("10.8.0.1/32")), "{:?}", s.routes);
    }
    // the loopback sockets are taken (a second bind fails), and nothing listens on a non-loopback address
    assert!(std::net::UdpSocket::bind(("127.0.0.1", port)).is_err());
    assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_err());
    r.mgr.set_enabled(&id, false, NOW + 1).unwrap();
    assert!(r.sys.clean());
    // the port is given back when the socket loop has been joined; wait for it (at most 10 s) instead of one try, and say who holds it if it stays taken
    let t0 = std::time::Instant::now();
    loop {
        match std::net::UdpSocket::bind(("127.0.0.1", port)) {
            Ok(_) => break,
            Err(e) if t0.elapsed() > std::time::Duration::from_secs(10) => panic!("the port is free after disable: {e}; {}", crate::testdir::describe_port(port)),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
}

#[test]
fn domain_rules_without_a_vpn_dns_server_do_not_connect_when_fail_closed_is_on() {
    let port = free_port();
    let r = rig_sockets("hybrid-nodns", port);
    let c = conf("0.0.0.0/0, ::/0", "vpn.example.com:51820").replace("DNS = 10.8.0.1\n", "");
    let id = r.mgr.import("NoDns", c.as_bytes()).unwrap().profile.id;
    r.mgr.set_settings(&id, Settings::hybrid(FilterSet::parse("*.corp.example.com").unwrap()), NOW).unwrap();
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    let st = r.mgr.status(&id);
    assert!(st.error.as_deref().unwrap_or("").contains("DNS"), "{st:?}");
    assert!(r.sys.clean() || r.sys.st.lock().unwrap().guard.is_some(), "only the guard may stay");
    std::net::UdpSocket::bind(("127.0.0.1", port)).expect("nothing was left listening");
}

#[test]
fn a_dropped_tunnel_stops_the_forwarder_and_a_reconnect_listens_again() {
    let port = free_port();
    let r = rig_sockets("hybrid-drop", port);
    let id = import_full(&r, "Office");
    r.mgr.set_settings(&id, Settings::hybrid(FilterSet::parse("*.corp.example.com").unwrap()), NOW).unwrap();
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    hs(&r, NOW as i64);
    r.mgr.tick(NOW + 1);
    assert!(matches!(r.mgr.status(&id).state, State::Connected { .. }));
    // the interface disappears: the watcher tears the tunnel parts down, which stops the loop
    r.sys.st.lock().unwrap().links.clear();
    r.mgr.tick(NOW + 2);
    // the watcher stops the forwarder thread, which gives the port back when it has noticed: wait for it (at most 10 s) instead of one try
    let t0 = std::time::Instant::now();
    let freed = loop {
        match std::net::UdpSocket::bind(("127.0.0.1", port)) {
            Ok(s) => break s,
            Err(e) if t0.elapsed() > std::time::Duration::from_secs(10) => panic!("the forwarder stopped with the tunnel: {e}; {}", crate::testdir::describe_port(port)),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    };
    drop(freed);
    // later the back-off ends and the connect listens again
    r.mgr.tick(NOW + 400);
    assert!(std::net::UdpSocket::bind(("127.0.0.1", port)).is_err(), "{:?}", r.mgr.status(&id));
    r.mgr.stop_all();
}

#[test]
fn the_summary_for_the_top_bar_follows_the_connection_and_never_names_a_key_or_a_server() {
    use crate::vpn_mgr::Light;
    let r = rig("summary");
    assert!(r.mgr.summary().is_none(), "nothing enabled, nothing shown");
    let id = import_full(&r, "Office");
    assert!(r.mgr.summary().is_none(), "imported but not enabled");
    r.mgr.set_settings(&id, full_settings(true), NOW).unwrap();
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    assert_eq!(r.mgr.summary().unwrap().light, Light::Working);
    hs(&r, NOW as i64);
    r.mgr.tick(NOW + 1);
    let s = r.mgr.summary().unwrap();
    assert_eq!(s.light, Light::Secure);
    assert!(s.line.contains("Office") && s.line.contains("FULL"));
    assert!(!s.line.contains("203.0.113.9") && !s.line.contains(&key(0x11)));
    // the interface vanishes: the kill switch stays, the lock says traffic is blocked
    r.sys.st.lock().unwrap().links.clear();
    r.mgr.tick(NOW + 2);
    assert_eq!(r.mgr.summary().unwrap().light, Light::Blocked, "{:?}", r.mgr.summary());
    // without a guard the honest light is "open"
    r.mgr.set_enabled(&id, false, NOW + 3).unwrap();
    r.mgr.set_settings(&id, full_settings(false), NOW + 3).unwrap();
    r.sys.fail("link add");
    r.mgr.set_enabled(&id, true, NOW + 4).unwrap();
    assert_eq!(r.mgr.summary().unwrap().light, Light::Open);
}

#[test]
fn the_startup_registry_hears_connecting_connected_and_down_without_a_key_or_a_server_in_the_words() {
    use crate::startup::{Registry, StartupState};
    let reg = Registry::new(|| 100);
    let r = rig("startup-report");
    r.mgr.report_startup(&reg);
    assert_eq!(reg.state("vpn"), Some(StartupState::Disabled), "nothing enabled");
    let id = import_full(&r, "Office");
    r.mgr.set_settings(&id, full_settings(true), NOW).unwrap();
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    r.mgr.report_startup(&reg);
    assert!(matches!(reg.state("vpn"), Some(StartupState::Starting { .. })), "{:?}", reg.state("vpn"));
    hs(&r, NOW as i64);
    r.mgr.tick(NOW + 1);
    r.mgr.report_startup(&reg);
    assert_eq!(reg.state("vpn"), Some(StartupState::Ready));
    r.sys.st.lock().unwrap().links.clear();
    r.mgr.tick(NOW + 2);
    r.mgr.report_startup(&reg);
    let Some(StartupState::Failed(why)) = reg.state("vpn") else { panic!("{:?}", reg.state("vpn")) };
    assert!(why.contains("Office") && !why.contains("203.0.113.9") && !why.contains(&key(0x11)), "{why}");
    r.mgr.stop_all();
}

#[test]
fn the_firewall_line_names_a_kill_switch_or_a_fail_closed_guard_only_while_it_is_in_place() {
    use iyakki_vpn::profile::{Mode, Settings};
    let r = rig("guard-line");
    let id = import_full(&r, "Office");
    assert_eq!(r.mgr.guard_line(), None, "nothing enabled");
    r.mgr.set_settings(&id, Settings { kill_switch: true, ..Settings::full() }, NOW).unwrap();
    assert_eq!(r.mgr.guard_line(), None, "set but not enabled: no guard yet");
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    let l = r.mgr.guard_line().expect("the guard is in");
    assert!(l.starts_with("VPN kill switch is active (Office)") && l.contains("local network keeps working"), "{l}");
    // the guard stays after the tunnel drops, and so does the line
    hs(&r, NOW as i64);
    r.mgr.tick(NOW + 1);
    r.sys.st.lock().unwrap().links.clear();
    r.mgr.tick(NOW + 2);
    assert!(r.mgr.guard_line().is_some());
    // disabling takes the guard and the line away
    r.mgr.set_enabled(&id, false, NOW + 3).unwrap();
    assert_eq!(r.mgr.guard_line(), None);
    // FULL without the kill switch has no guard to name
    r.mgr.set_settings(&id, Settings { kill_switch: false, ..Settings::full() }, NOW + 4).unwrap();
    r.mgr.set_enabled(&id, true, NOW + 5).unwrap();
    assert_eq!(r.mgr.guard_line(), None);
    assert_eq!(r.mgr.profiles()[0].settings.mode, Mode::Full);
}

#[test]
fn the_guard_line_says_how_many_more_profiles_hold_a_guard() {
    use iyakki_vpn::profile::Settings;
    let r = rig("guard-line2");
    let id = import_full(&r, "Office");
    r.mgr.set_settings(&id, Settings { kill_switch: true, ..Settings::full() }, NOW).unwrap();
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    let mut rows = r.mgr.rows();
    let mut second = rows[0].clone();
    second.profile.name = "Lab".into();
    rows.push(second);
    let l = crate::vpn_mgr::guard_line_of(&rows).unwrap();
    assert!(l.contains("(Office and 1 more)"), "{l}");
}

// ------------------------------------------------------------------ OpenVPN (F-VPN-06)

const OVPN_TEXT: &str = "client\ndev tun\nproto udp\nremote vpn.example.com 1194\nnobind\nremote-cert-tls server\n<ca>\n-----BEGIN CERTIFICATE-----\nFAKECA\n-----END CERTIFICATE-----\n</ca>\n<key>\n-----BEGIN PRIVATE KEY-----\nFAKE-KEY-CANARY\n-----END PRIVATE KEY-----\n</key>\n";

fn ovpn_rig(tag: &str, lines: &[&str]) -> (Rig, Arc<crate::vpn_openvpn::fakes::ScriptLauncher>) {
    let r = rig(tag);
    let l = Arc::new(crate::vpn_openvpn::fakes::ScriptLauncher::new(lines));
    let audit: AuditFn = Arc::new(|_, _| {});
    let mgr = Arc::new(Manager::new(&r.dir, r.vault.clone(), r.sys.clone(), audit).with_openvpn(l.clone()));
    (Rig { mgr, ..r }, l)
}

#[test]
fn an_openvpn_profile_connects_through_the_checked_pushed_settings_and_disable_undoes_all_of_it() {
    use crate::vpn_openvpn::fakes::{PUSH, UP};
    let (r, l) = ovpn_rig("ovpn-up", &["OpenVPN starting", PUSH, UP]);
    let id = r.mgr.import("Home", OVPN_TEXT.as_bytes()).unwrap().profile.id;
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    {
        let s = r.sys.st.lock().unwrap();
        assert!(s.routes.contains(&"203.0.113.9/32 via 192.168.1.1 dev eth0".to_string()), "the server stays reachable: {:?}", s.routes);
        assert!(s.routes.contains(&format!("0.0.0.0/1 dev {IFNAME}")) && s.routes.contains(&format!("128.0.0.0/1 dev {IFNAME}")), "{:?}", s.routes);
        assert!(s.resolv.as_deref().unwrap().contains("nameserver 10.8.0.1"));
    }
    {
        let c = l.configs.lock().unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].1, IFNAME);
        assert!(c[0].0.contains("FAKE-KEY-CANARY"), "the secret block reaches the program (only through its private file)");
    }
    assert!(!r.sys.calls().join("|").contains("FAKE-KEY-CANARY"), "no secret in any command");
    // the program makes the tunnel interface itself: the stand-in does it here
    r.sys.st.lock().unwrap().links.insert(IFNAME.to_string());
    r.mgr.tick(NOW + 5);
    assert!(matches!(r.mgr.status(&id).state, State::Connected { .. }), "{:?}", r.mgr.status(&id));
    r.mgr.set_enabled(&id, false, NOW + 10).unwrap();
    assert!(*l.stopped.lock().unwrap(), "the program is stopped first");
    assert!(r.sys.clean(), "{:?}", r.sys.calls());
}

#[test]
fn a_refused_login_stops_the_program_undoes_everything_and_is_not_retried() {
    let (r, l) = ovpn_rig("ovpn-auth", &["AUTH: Received control message: AUTH_FAILED"]);
    let id = r.mgr.import("Home", OVPN_TEXT.as_bytes()).unwrap().profile.id;
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    let st = r.mgr.status(&id);
    assert!(st.needs_you && st.retry_at.is_none() && st.error.as_deref().unwrap().contains("user name or password"), "{st:?}");
    assert!(*l.stopped.lock().unwrap());
    assert!(r.sys.clean());
}

#[test]
fn a_hostile_push_is_refused_before_anything_is_changed() {
    let push = "PUSH: Received control message: 'PUSH_REPLY,ifconfig 10.8.0.6 255.255.255.0,route 10.43.0.0 255.255.0.0'";
    let (r, _l) = ovpn_rig("ovpn-bad", &[push, crate::vpn_openvpn::fakes::UP]);
    let id = r.mgr.import("Home", OVPN_TEXT.as_bytes()).unwrap().profile.id;
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    let st = r.mgr.status(&id);
    assert!(st.needs_you && st.error.is_some(), "a route into the cluster network is refused: {st:?}");
    assert!(r.sys.clean());
}

#[test]
fn when_the_program_ends_the_pieces_go_and_a_retry_is_planned() {
    use crate::vpn_openvpn::fakes::{PUSH, UP};
    let (r, l) = ovpn_rig("ovpn-drop", &[PUSH, UP]);
    let id = r.mgr.import("Home", OVPN_TEXT.as_bytes()).unwrap().profile.id;
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    r.sys.st.lock().unwrap().links.insert(IFNAME.to_string());
    *l.stopped.lock().unwrap() = true; // the program died
    r.mgr.tick(NOW + 20);
    let st = r.mgr.status(&id);
    assert!(st.error.as_deref().unwrap().contains("program ended") && st.retry_at.is_some(), "{st:?}");
    assert!(r.sys.clean());
}

#[test]
fn a_missing_program_is_said_in_plain_words_and_nothing_changes() {
    let (r, _) = {
        let r = rig("ovpn-none");
        let mut l = crate::vpn_openvpn::fakes::ScriptLauncher::new(&[]);
        l.present = false;
        let audit: AuditFn = Arc::new(|_, _| {});
        let mgr = Arc::new(Manager::new(&r.dir, r.vault.clone(), r.sys.clone(), audit).with_openvpn(Arc::new(l)));
        (Rig { mgr, ..r }, ())
    };
    let id = r.mgr.import("Home", OVPN_TEXT.as_bytes()).unwrap().profile.id;
    assert!(r.mgr.set_enabled(&id, true, NOW).unwrap_err().contains("not downloaded"));
    assert!(!r.mgr.is_enabled(&id) && r.sys.clean());
}

#[test]
fn a_profile_that_asks_for_a_login_gets_it_from_the_vault_only_and_not_before() {
    use crate::vpn_openvpn::fakes::{PUSH, UP};
    let (r, l) = ovpn_rig("ovpn-cred", &[PUSH, UP]);
    let text = format!("{OVPN_TEXT}auth-user-pass\n");
    let id = r.mgr.import("Work", text.as_bytes()).unwrap().profile.id;
    assert!(!r.mgr.has_credentials(&id));
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    let st = r.mgr.status(&id);
    assert!(st.needs_you && st.error.as_deref().unwrap().contains("user name and password"), "{st:?}");
    assert!(l.configs.lock().unwrap().is_empty(), "the program was not started");
    assert!(r.mgr.set_credentials(&id, "amy", "line\nbreak").is_err() && r.mgr.set_credentials(&id, "", "x").is_err());
    r.mgr.set_credentials(&id, "amy", "pa55word-CANARY").unwrap();
    r.mgr.retry(&id, NOW + 1);
    assert!(l.configs.lock().unwrap()[0].0.contains("pa55word-CANARY"), "the program gets it in its private config");
    let all = format!("{:?}{:?}", r.mgr.status(&id), r.sys.calls());
    assert!(!all.contains("pa55word-CANARY"), "never in the status or a command");
    assert!(!std::fs::read_to_string(r.dir.join("profiles.json")).unwrap().contains("pa55word-CANARY"));
    // a WireGuard profile does not take one
    let wg = import_full(&r, "Office");
    assert!(r.mgr.set_credentials(&wg, "a", "b").is_err());
    r.mgr.remove(&id).unwrap();
    assert!(!r.mgr.has_credentials(&id), "removed with the profile");
}

#[test]
fn chosen_accounts_are_sent_through_the_tunnel_by_policy_and_it_is_taken_out_with_the_tunnel() {
    let r = rig("policy-uids");
    let id = import_full(&r, "Office");
    let mut s = Settings::hybrid(FilterSet::parse("198.51.100.0/24").unwrap());
    s.policy_uids = vec![1001, 1002];
    r.mgr.set_settings(&id, s, NOW).unwrap();
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    {
        let st = r.sys.st.lock().unwrap();
        let (nft, add) = st.policy.clone().expect("the policy is in place");
        assert!(nft.contains("meta skuid { 1001, 1002 }") && nft.contains("198.51.100.0/24"), "{nft}");
        assert!(nft.contains("203.0.113.9/32"), "the VPN server is always left out: {nft}");
        assert!(!add.is_empty());
    }
    let c = r.sys.calls();
    assert!(c.iter().position(|x| x.contains("policy install")).unwrap() > c.iter().position(|x| x.contains("198.51.100.0/24")).unwrap(), "after the routes: {c:?}");
    r.mgr.set_enabled(&id, false, NOW + 1).unwrap();
    assert!(r.sys.st.lock().unwrap().policy.is_none() && r.sys.clean());
}

#[test]
fn accounts_by_policy_are_refused_in_full_mode_for_account_zero_and_too_many() {
    let r = rig("policy-uids-bad");
    let id = import_full(&r, "Office");
    let mut s = Settings::hybrid(FilterSet::parse("198.51.100.0/24").unwrap());
    s.policy_uids = vec![0];
    assert!(r.mgr.set_settings(&id, s.clone(), NOW).unwrap_err().contains("administrator"));
    s.policy_uids = (1000..1040).collect();
    assert!(r.mgr.set_settings(&id, s, NOW).is_err());
    let mut f = Settings::full();
    f.policy_uids = vec![1001];
    assert!(r.mgr.set_settings(&id, f, NOW).is_err());
}

#[test]
fn a_failed_policy_install_stops_a_fail_closed_connect_and_leaves_nothing() {
    let r = rig("policy-fail");
    let id = import_full(&r, "Office");
    let mut s = Settings::hybrid(FilterSet::parse("198.51.100.0/24").unwrap());
    s.policy_uids = vec![1001];
    r.mgr.set_settings(&id, s, NOW).unwrap();
    r.sys.st.lock().unwrap().fail_on.push("policy install".into());
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    assert!(r.mgr.status(&id).error.is_some_and(|e| e.contains("policy routing")), "{:?}", r.mgr.status(&id));
    assert!(r.sys.st.lock().unwrap().policy.is_none());
}

#[test]
fn roaming_to_another_network_and_a_resume_from_sleep_build_the_tunnel_again() {
    let r = rig("roam");
    let id = import_full(&r, "Office");
    r.mgr.set_enabled(&id, true, NOW).unwrap();
    hs(&r, NOW as i64);
    let before = r.sys.calls().iter().filter(|c| c.contains("wg configure") || c.contains("wg_configure")).count();
    r.mgr.tick(NOW + 5);
    assert_eq!(r.sys.calls().iter().filter(|c| c.contains("wg configure") || c.contains("wg_configure")).count(), before, "no change, no rebuild");
    // the default route moves to another interface
    r.sys.st.lock().unwrap().net.physical_if = "wlan9".into();
    hs(&r, (NOW + 10) as i64);
    r.mgr.tick(NOW + 10);
    assert!(r.log.lock().unwrap().iter().any(|l| l.starts_with("vpn.roam")), "{:?}", r.log.lock().unwrap());
    // a heartbeat far later than the last one is a resume
    hs(&r, (NOW + 1000) as i64);
    r.mgr.tick(NOW + 1000);
    assert!(r.log.lock().unwrap().iter().any(|l| l.starts_with("vpn.resume")), "{:?}", r.log.lock().unwrap());
    r.mgr.set_enabled(&id, false, NOW + 1001).unwrap();
    assert!(r.sys.clean());
}
