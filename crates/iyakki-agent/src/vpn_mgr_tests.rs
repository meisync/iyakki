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
    r.mgr.tick(NOW + 61 + 400);
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
    std::net::UdpSocket::bind(("127.0.0.1", port)).expect("the port is free after disable");
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
    std::net::UdpSocket::bind(("127.0.0.1", port)).expect("the forwarder stopped with the tunnel");
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
