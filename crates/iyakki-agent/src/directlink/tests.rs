//! Direct links against the made-up machine: link-local choice and ARP, the private pair, the beacon parser, Thunderbolt sysfs and
//! authorization per security level, the manager's bring-up, every step of the check list, the speed numbers and the gadget plan.

use std::net::Ipv4Addr;
use std::path::PathBuf;

use super::beacon::{Beacon, BeaconError};
use super::checks::{self, Action, State};
use super::fake::{iface, FakeEnv};
use super::linklocal;
use super::manager::Manager;
use super::tb::{self, Auth, Device, Kind, TbView};
use super::*;

const MAC_A: &str = "02:11:22:33:44:55";
const MAC_B: &str = "02:aa:bb:cc:dd:ee";

fn mac(s: &str) -> [u8; 6] {
    linklocal::parse_mac(s).unwrap()
}

fn tmp(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let p = std::env::temp_dir().join(format!("iyk-dl-{tag}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn put(root: &std::path::Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn beacon(host: &str, mac: &str, id: &str, dev_on: bool, pairing: bool) -> Beacon {
    Beacon { host: host.into(), version: "0.1.0-test".into(), role: if dev_on { "target" } else { "host" }.into(), dev_on, pairing_open: pairing, mac: mac.into(), id: id.into(), port: 7979 }
}

fn xdomain() -> Device {
    Device { name: "0-1".into(), kind: Kind::XDomain, authorized: None, vendor: "IYAKKI".into(), model: "other-box".into(), unique_id: "aa-bb".into(), rx_gbps: Some(20.0), tx_gbps: Some(20.0), lanes: Some(2) }
}

fn router(auth: u8) -> Device {
    Device { name: "0-3".into(), kind: Kind::Router, authorized: Some(auth), vendor: "Acme".into(), model: "Dock".into(), unique_id: "cc-dd".into(), rx_gbps: None, tx_gbps: None, lanes: None }
}

fn tb_view(security: &str, devices: Vec<Device>) -> TbView {
    TbView { controller_present: true, driver_loaded: true, security: security.into(), devices }
}

// ---------------------------------------------------------------- link-local addresses and ARP

#[test]
fn link_local_candidates_are_stable_in_range_and_differ_by_attempt() {
    let m = mac(MAC_A);
    assert_eq!(linklocal::candidate(&m, 0), linklocal::candidate(&m, 0));
    let mut seen = std::collections::BTreeSet::new();
    for attempt in 0..200 {
        let ip = linklocal::candidate(&m, attempt);
        let o = ip.octets();
        assert_eq!((o[0], o[1]), (169, 254));
        assert!((1..=254).contains(&o[2]), "{ip}: the first and last 256 addresses are reserved");
        seen.insert(ip);
    }
    assert!(seen.len() > 190, "attempts must give different addresses: {}", seen.len());
    assert_ne!(linklocal::candidate(&mac(MAC_A), 0), linklocal::candidate(&mac(MAC_B), 0));
    assert!(linklocal::is_link_local(Ipv4Addr::new(169, 254, 3, 4)) && !linklocal::is_link_local(Ipv4Addr::new(10, 0, 0, 1)));
}

#[test]
fn acquire_moves_on_when_an_address_is_taken_and_gives_up_when_all_are() {
    let m = mac(MAC_A);
    let env = FakeEnv::new();
    let first = linklocal::candidate(&m, 0);
    let second = linklocal::candidate(&m, 1);
    env.with(|s| s.taken = vec![first]);
    assert_eq!(linklocal::acquire(&env, "thunderbolt0", m, 10), Ok(second), "the second candidate is used");
    assert_eq!(env.with(|s| s.probes.clone()), vec![("thunderbolt0".to_string(), first), ("thunderbolt0".to_string(), second)]);
    // all ten taken
    env.with(|s| s.taken = (0..10).map(|a| linklocal::candidate(&m, a)).collect());
    let e = linklocal::acquire(&env, "thunderbolt0", m, 10).unwrap_err();
    assert!(e.contains("10 link-local addresses were all taken"), "{e}");
    // a probe that cannot be sent is an error, not a free address
    env.with(|s| s.probe_fails = true);
    assert!(linklocal::acquire(&env, "thunderbolt0", m, 10).unwrap_err().contains("could not probe"));
}

#[test]
fn the_private_pair_is_agreed_from_the_mac_addresses_and_never_collides() {
    let (a, b) = (mac(MAC_A), mac(MAC_B));
    let (a_mine, a_peer) = linklocal::private_pair(&a, &b).unwrap();
    let (b_mine, b_peer) = linklocal::private_pair(&b, &a).unwrap();
    assert_eq!((a_mine, a_peer), (Ipv4Addr::new(10, 77, 0, 1), Ipv4Addr::new(10, 77, 0, 2)), "the lower MAC is .1");
    assert_eq!((b_mine, b_peer), (a_peer, a_mine), "each end sees the other's choice");
    assert_ne!(a_mine, b_mine);
    assert_eq!(linklocal::private_pair(&a, &a), None, "equal MAC addresses cannot be agreed on");
}

#[test]
fn arp_frames_roundtrip_and_conflicts_are_told_from_our_own_packets() {
    let (a, b) = (mac(MAC_A), mac(MAC_B));
    let ip = Ipv4Addr::new(169, 254, 9, 9);
    let f = linklocal::probe_frame(&a, ip);
    assert_eq!(f.len(), 42);
    let p = linklocal::parse_arp(&f).unwrap();
    assert_eq!((p.op, p.sha, p.spa, p.tpa), (1, a, Ipv4Addr::UNSPECIFIED, ip));
    assert_eq!(linklocal::conflicts(&p, &a, ip), None, "our own probe is not a conflict");
    // somebody else probes for the same address at the same time
    let other = linklocal::parse_arp(&linklocal::probe_frame(&b, ip)).unwrap();
    assert_eq!(linklocal::conflicts(&other, &a, ip), Some(b));
    // somebody else uses it (a reply, or an announcement)
    let ann = linklocal::parse_arp(&linklocal::arp_frame(&b, ip, ip)).unwrap();
    assert_eq!(linklocal::conflicts(&ann, &a, ip), Some(b));
    // a probe for another address is none of our business
    let elsewhere = linklocal::parse_arp(&linklocal::probe_frame(&b, Ipv4Addr::new(169, 254, 1, 1))).unwrap();
    assert_eq!(linklocal::conflicts(&elsewhere, &a, ip), None);
    // short, wrong-type and garbage frames are refused, never a panic
    assert!(linklocal::parse_arp(&f[..41]).is_none() && linklocal::parse_arp(&[]).is_none());
    let mut bad = f;
    bad[12] = 0x86;
    assert!(linklocal::parse_arp(&bad).is_none());
    assert_eq!(linklocal::parse_mac("02:11:22:33:44"), None);
    assert_eq!(linklocal::parse_mac("02:11:22:33:44:5"), None);
    assert_eq!(linklocal::parse_mac("zz:11:22:33:44:55"), None);
    assert_eq!(linklocal::mac_text(&a), MAC_A);
}

// ---------------------------------------------------------------- the beacon

#[test]
fn a_beacon_roundtrips_and_malformed_packets_never_panic_or_pass() {
    let b = beacon("other-box", MAC_B, "00112233445566778899aabbccddeeff", true, true);
    let bytes = b.encode();
    assert!(bytes.len() < beacon::MAX_LEN);
    assert_eq!(Beacon::parse(&bytes), Ok(b.clone()));
    assert!(!String::from_utf8(bytes.clone()).unwrap().to_lowercase().contains("code"), "never a pairing code, only whether one is open");
    // size
    assert_eq!(Beacon::parse(&vec![b'a'; beacon::MAX_LEN + 1]), Err(BeaconError::TooLong));
    // magic, text, lines
    assert_eq!(Beacon::parse(b"hello"), Err(BeaconError::Magic));
    assert_eq!(Beacon::parse(&[0xff, 0xfe, 0x00]), Err(BeaconError::NotText));
    assert_eq!(Beacon::parse(b"IYAKKI-DEV 1\nhost=a\x07b\n"), Err(BeaconError::NotText));
    let many = format!("IYAKKI-DEV 1\n{}", "x=1\n".repeat(30));
    assert_eq!(Beacon::parse(many.as_bytes()), Err(BeaconError::TooManyLines));
    // fields
    let text = String::from_utf8(bytes.clone()).unwrap();
    for (from, to, what) in [("host=other-box", "host=bad host!", "host"), ("host=other-box", "host=", "host"), ("role=target", "role=root", "role"), ("dev=1", "dev=2", "dev"), (MAC_B, "02:aa", "mac"), ("port=7979", "port=80", "port"), ("port=7979", "port=99999", "port"), ("id=00112233445566778899aabbccddeeff", "id=zz", "id")] {
        assert_eq!(Beacon::parse(text.replacen(from, to, 1).as_bytes()), Err(BeaconError::Field(what)), "{from} -> {to}");
    }
    let long_host = format!("host={}", "h".repeat(64));
    assert!(Beacon::parse(text.replacen("host=other-box", &long_host, 1).as_bytes()).is_err());
    assert_eq!(Beacon::parse(text.replace("host=other-box\n", "").as_bytes()), Err(BeaconError::Missing("host")));
    // unknown keys are ignored (a newer version may add some)
    assert!(Beacon::parse(format!("{text}future=1\n").as_bytes()).is_ok());
    // noise: a few thousand pseudo-random packets and truncations of a good one
    let mut x: u64 = 0x1234_5678_9abc_def1;
    for n in 0..3000 {
        let len = (n % 700) as usize;
        let junk: Vec<u8> = (0..len).map(|_| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x as u8 }).collect();
        let _ = Beacon::parse(&junk);
    }
    for cut in 0..bytes.len() {
        let _ = Beacon::parse(&bytes[..cut]);
    }
}

// ---------------------------------------------------------------- Thunderbolt in sysfs

#[test]
fn the_thunderbolt_tree_is_read_from_sysfs_files() {
    let root = tmp("tb");
    // a Thunderbolt 3 controller (Intel NHI) and an unrelated PCI device
    put(&root, "bus/pci/devices/0000:06:00.0/class", "0x088000\n");
    put(&root, "bus/pci/devices/0000:06:00.0/vendor", "0x8086\n");
    put(&root, "bus/pci/devices/0000:00:02.0/class", "0x030000\n");
    put(&root, "bus/pci/devices/0000:00:02.0/vendor", "0x8086\n");
    let view = tb::read_view(&root);
    assert!(view.controller_present && !view.driver_loaded, "the controller is on the bus but the driver is not loaded: {view:?}");
    // the driver is loaded
    put(&root, "bus/thunderbolt/devices/domain0/security", "user\n");
    put(&root, "bus/thunderbolt/devices/0-0/authorized", "1\n");
    put(&root, "bus/thunderbolt/devices/0-0/vendor_name", "Apple\n");
    put(&root, "bus/thunderbolt/devices/0-0/unique_id", "0000-1111\n");
    put(&root, "bus/thunderbolt/devices/0-1/unique_id", "aaaa-bbbb\n");
    put(&root, "bus/thunderbolt/devices/0-1/vendor_name", "IYAKKI\n");
    put(&root, "bus/thunderbolt/devices/0-1/device_name", "other-box\n");
    put(&root, "bus/thunderbolt/devices/0-1/rx_speed", "20.0 Gb/s\n");
    put(&root, "bus/thunderbolt/devices/0-1/tx_speed", "20.0 Gb/s\n");
    put(&root, "bus/thunderbolt/devices/0-1/rx_lanes", "2\n");
    put(&root, "bus/thunderbolt/devices/0-1.1/prtcid", "1\n");
    put(&root, "bus/thunderbolt/devices/0-2/authorized", "0\n");
    let view = tb::read_view(&root);
    assert!(view.controller_present && view.driver_loaded);
    assert_eq!(view.security, "user");
    let peer = view.peer_computer().expect("a host-to-host connection");
    assert_eq!((peer.name.as_str(), peer.model.as_str(), peer.rx_gbps, peer.tx_gbps, peer.lanes), ("0-1", "other-box", Some(20.0), Some(20.0), Some(2)));
    assert_eq!(view.devices.iter().filter(|d| d.kind == Kind::Service).count(), 1);
    assert_eq!(view.pending().iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), vec!["0-2"], "only the router with authorized=0 waits");
    // USB4 host interface by class
    let usb4 = tmp("usb4");
    put(&usb4, "bus/pci/devices/0000:00:0d.2/class", "0x0c0340\n");
    put(&usb4, "bus/pci/devices/0000:00:0d.2/vendor", "0x8086\n");
    assert!(tb::read_view(&usb4).controller_present);
    // nothing at all: no panic, nothing present
    assert_eq!(tb::read_view(&tmp("none")), TbView::default());
    assert_eq!(tb::parse_gbps("10.0 Gb/s"), Some(10.0));
    assert_eq!(tb::parse_gbps("fast"), None);
    assert_eq!(tb::parse_gbps("NaN Gb/s"), None);
    for d in [root, usb4] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn authorization_follows_the_security_level() {
    let mk = |level: &str, auth: u8, key: bool| tb::auth_plan(level, &router(auth), key);
    assert_eq!(mk("user", 0, false), Auth::Approve);
    assert_eq!(mk("none", 0, false), Auth::Approve, "if the kernel left one unapproved at none, approving is harmless");
    assert_eq!(mk("secure", 0, false), Auth::ApproveWithNewKey);
    assert_eq!(mk("secure", 0, true), Auth::Challenge);
    for l in ["dponly", "usbonly", "nopcie"] {
        assert!(matches!(mk(l, 0, false), Auth::NotNeeded(_)), "{l}");
    }
    assert!(matches!(mk("user", 1, false), Auth::NotNeeded("already approved")));
    assert!(matches!(tb::auth_plan("user", &xdomain(), false), Auth::NotNeeded(_)), "a host-to-host connection needs no approval");
    assert!(tb::security_words("user").contains("Authorize") && tb::security_words("???").contains("unknown"));
}

#[test]
fn authorize_writes_the_right_things_and_keeps_the_key() {
    let env = FakeEnv::new();
    env.with(|s| s.tb = tb_view("user", vec![router(0)]));
    assert!(tb::authorize(&env, "0-3").unwrap().contains("approved"));
    assert_eq!(env.calls(), vec!["authorize 0-3 1"]);
    // secure, first time: a key is made, stored, written, then 1
    env.with(|s| {
        s.tb = tb_view("secure", vec![router(0)]);
        s.calls.clear();
    });
    tb::authorize(&env, "0-3").unwrap();
    assert_eq!(env.calls(), vec!["authorize 0-3 1 key=abab"]);
    assert_eq!(env.tb_key("cc-dd").unwrap().len(), 64);
    // secure, later: the stored key is challenged (2)
    env.with(|s| {
        s.tb = tb_view("secure", vec![router(0)]);
        s.calls.clear();
    });
    tb::authorize(&env, "0-3").unwrap();
    assert_eq!(env.calls(), vec!["authorize 0-3 2 key=abab"]);
    // a level without PCIe tunnels: nothing is written
    env.with(|s| {
        s.tb = tb_view("dponly", vec![router(0)]);
        s.calls.clear();
    });
    assert!(tb::authorize(&env, "0-3").unwrap().contains("Nothing to do"));
    assert!(env.calls().is_empty());
    // a device that went away, a name that is not one, a write the kernel refuses
    assert!(tb::authorize(&env, "0-9").unwrap_err().contains("not connected"));
    env.with(|s| s.tb = tb_view("user", vec![Device { name: "../x".into(), ..router(0) }]));
    assert!(tb::authorize(&env, "../x").is_err());
    env.with(|s| {
        s.tb = tb_view("user", vec![router(0)]);
        s.fail_auth = true;
    });
    assert!(tb::authorize(&env, "0-3").unwrap_err().contains("not permitted"));
}

// ---------------------------------------------------------------- classification and the manager

#[test]
fn interfaces_are_classified_by_driver_and_name() {
    let c = |name: &str, drv: &str, m: &str, udc: bool| classify(&iface(name, drv, m, true, true), udc);
    assert_eq!(c("thunderbolt0", "thunderbolt-net", MAC_A, false), Some(Transport::Thunderbolt));
    assert_eq!(c("eth3", "thunderbolt_net", MAC_A, false), Some(Transport::Thunderbolt));
    assert_eq!(c("enx0011", "plusb", MAC_A, false), Some(Transport::BridgeCable));
    assert_eq!(c("usb1", "net1080", MAC_A, false), Some(Transport::BridgeCable));
    assert_eq!(c("enx0011", "cdc_subset", MAC_A, false), Some(Transport::BridgeCable));
    assert_eq!(c("usb0", "", MAC_A, true), Some(Transport::Gadget), "the gadget side has no driver link");
    assert_eq!(c("usb0", "", MAC_A, false), None, "without a device controller usb0 is nothing special");
    assert_eq!(c("enx0011", "cdc_ncm", MAC_A, false), Some(Transport::UsbNet));
    assert_eq!(c("eth0", "e1000e", MAC_A, false), None, "an ordinary Ethernet card is never a direct link");
    assert_eq!(c("wlan0", "iwlwifi", MAC_A, false), None);
    assert_eq!(c("enx0011", "cdc_ncm", "ac:de:48:00:11:22", false), None, "the T2 chip's own link");
    assert_eq!(c("lo", "", "00:00:00:00:00:00", true), None);
}

fn tb_up_env() -> FakeEnv {
    let env = FakeEnv::new();
    env.with(|s| {
        s.ifaces = vec![iface("eth0", "e1000e", "52:54:00:00:00:01", true, true), iface("thunderbolt0", "thunderbolt-net", MAC_A, false, true)];
        s.tb = tb_view("user", vec![xdomain()]);
    });
    env
}

#[test]
fn a_thunderbolt_link_comes_up_without_dhcp_and_the_ordinary_card_is_left_alone() {
    let env = tb_up_env();
    let mut m = Manager::new();
    let snap = m.tick(&env, &Config::default());
    let calls = env.calls();
    assert_eq!(calls[0], "up thunderbolt0");
    assert_eq!(calls[1], "noforward thunderbolt0", "nothing is forwarded to or from the cable");
    let ll = linklocal::candidate(&mac(MAC_A), 0);
    assert!(calls.contains(&format!("add thunderbolt0:0 {ll}/16")), "{calls:?}");
    assert!(calls.contains(&"announce thunderbolt0".to_string()));
    assert!(calls.iter().all(|c| !c.contains("eth0")), "eth0 is not touched: {calls:?}");
    assert_eq!(m.zone(), vec!["thunderbolt0"]);
    assert_eq!(snap.links.len(), 1);
    assert_eq!((snap.links[0].link_local, snap.links[0].speed_mbps, snap.links[0].transport), (Some(ll), Some(10000), Transport::Thunderbolt));
    // the first beacon went out on the cable, and says who we are
    let sent = env.with(|s| s.sent.clone());
    assert_eq!(sent.len(), 1);
    let b = Beacon::parse(&sent[0].2).unwrap();
    assert_eq!((b.host.as_str(), b.mac.as_str()), ("dev-pc", MAC_A));
    // a second tick does not probe or add again
    let before = env.calls().len();
    m.tick(&env, &Config::default());
    assert_eq!(env.calls().len(), before);
}

#[test]
fn a_taken_link_local_address_is_replaced_and_a_cable_without_link_waits() {
    let env = tb_up_env();
    let m0 = mac(MAC_A);
    env.with(|s| s.taken = vec![linklocal::candidate(&m0, 0), linklocal::candidate(&m0, 1)]);
    let mut m = Manager::new();
    m.tick(&env, &Config::default());
    assert!(env.calls().contains(&format!("add thunderbolt0:0 {}/16", linklocal::candidate(&m0, 2))));
    // no cable link: switched up, forwarding off, but no address and no beacon
    let env = tb_up_env();
    env.with(|s| s.ifaces[1].carrier = false);
    let mut m = Manager::new();
    m.tick(&env, &Config::default());
    assert!(env.calls().iter().all(|c| !c.starts_with("add")) && env.with(|s| s.sent.is_empty()));
    // a failed address is retried later, not every two seconds
    let env = tb_up_env();
    env.with(|s| s.fail_add = true);
    let mut m = Manager::new();
    m.tick(&env, &Config::default());
    let probes = env.with(|s| s.probes.len());
    m.tick(&env, &Config::default());
    assert_eq!(env.with(|s| s.probes.len()), probes, "waits ten seconds");
    env.with(|s| {
        s.now += 11;
        s.fail_add = false;
    });
    m.tick(&env, &Config::default());
    assert!(env.calls().iter().any(|c| c.starts_with("add thunderbolt0:0")));
}

#[test]
fn an_ambiguous_usb_network_interface_waits_for_dhcp_and_lets_go_when_an_address_arrives() {
    let env = FakeEnv::new();
    env.with(|s| s.ifaces = vec![iface("enx001122", "cdc_ncm", MAC_A, true, true)]);
    let mut m = Manager::new();
    m.tick(&env, &Config::default());
    assert!(env.calls().is_empty() && m.zone().is_empty(), "DHCP gets its chance first");
    env.with(|s| s.now += DHCP_GRACE_SECS + 1);
    m.tick(&env, &Config::default());
    assert_eq!(m.zone(), vec!["enx001122"], "no DHCP answer: now it is a direct link");
    assert!(env.calls().iter().any(|c| c.starts_with("add enx001122:0 169.254.")));
    // a real address arrives (a late DHCP lease): the link-local one goes and the zone is left
    env.with(|s| s.ifaces[0].ipv4 = vec![Ipv4Addr::new(192, 168, 5, 20)]);
    m.tick(&env, &Config::default());
    assert!(m.zone().is_empty() && env.calls().iter().any(|c| c == "del enx001122:0"));
    // an interface that already has a real address from the start is never taken over
    let env = FakeEnv::new();
    env.with(|s| s.ifaces = vec![iface("enx001122", "cdc_ncm", MAC_A, true, true)]);
    env.with(|s| s.ifaces[0].ipv4 = vec![Ipv4Addr::new(192, 168, 5, 20)]);
    let mut m = Manager::new();
    m.tick(&env, &Config::default());
    assert!(env.calls().is_empty());
    // a Thunderbolt interface a person configured by hand is left alone too
    let env = tb_up_env();
    env.with(|s| s.ifaces[1].ipv4 = vec![Ipv4Addr::new(10, 1, 1, 1)]);
    let mut m = Manager::new();
    m.tick(&env, &Config::default());
    assert!(env.calls().is_empty() && m.zone().is_empty());
}

#[test]
fn the_manager_keeps_the_peer_table_ignores_its_own_packets_and_garbage_and_forgets_old_peers() {
    let env = tb_up_env();
    let mut m = Manager::new();
    m.tick(&env, &Config::default());
    let peer_ip = Ipv4Addr::new(169, 254, 77, 1);
    let good = beacon("other-box", MAC_B, "ffeeddccbbaa99887766554433221100", true, false).encode();
    let own = beacon("dev-pc", MAC_A, "00112233445566778899aabbccddeeff", false, false).encode();
    env.with(|s| s.inbox.insert("thunderbolt0".into(), vec![(peer_ip, good.clone()), (peer_ip, own), (peer_ip, b"garbage".to_vec()), (peer_ip, vec![0xff; 600])]));
    env.with(|s| s.now += 1);
    let snap = m.tick(&env, &Config::default());
    assert_eq!(snap.peers.len(), 1, "only the other computer: {snap:?}");
    assert_eq!((snap.peers[0].addr, snap.peers[0].beacon.host.as_str()), (peer_ip, "other-box"));
    // the same peer again updates the entry, it does not add one
    env.with(|s| s.inbox.insert("thunderbolt0".into(), vec![(Ipv4Addr::new(169, 254, 77, 2), good)]));
    env.with(|s| s.now += 1);
    let snap = m.tick(&env, &Config::default());
    assert_eq!((snap.peers.len(), snap.peers[0].addr), (1, Ipv4Addr::new(169, 254, 77, 2)));
    // silence for longer than the time to live
    env.with(|s| s.now += manager::PEER_TTL + 1);
    assert!(m.tick(&env, &Config::default()).peers.is_empty());
    // the peer table is bounded
    let flood: Vec<(Ipv4Addr, Vec<u8>)> = (0..60u32).map(|n| (peer_ip, beacon("p", MAC_B, &format!("{n:032x}"), true, false).encode())).collect();
    env.with(|s| s.inbox.insert("thunderbolt0".into(), flood));
    assert!(m.tick(&env, &Config::default()).peers.len() <= 16);
}

#[test]
fn the_fixed_private_pair_is_added_only_when_asked_and_only_after_the_peers_mac_is_known() {
    let env = tb_up_env();
    let cfg = Config { fixed_pair: true, ..Config::default() };
    let mut m = Manager::new();
    m.tick(&env, &cfg);
    assert!(env.calls().iter().all(|c| !c.contains("10.77")), "the peer is not known yet");
    env.with(|s| s.inbox.insert("thunderbolt0".into(), vec![(Ipv4Addr::new(169, 254, 77, 1), beacon("other-box", MAC_B, "ffeeddccbbaa99887766554433221100", true, false).encode())]));
    env.with(|s| s.now += 1);
    let snap = m.tick(&env, &cfg);
    // MAC_A (02:11..) is lower than MAC_B (02:aa..): this end is 10.77.0.1
    assert!(env.calls().contains(&"add thunderbolt0:1 10.77.0.1/30".to_string()), "{:?}", env.calls());
    assert_eq!(snap.links[0].private, Some(Ipv4Addr::new(10, 77, 0, 1)));
    // switched off in Settings: it goes again
    m.tick(&env, &Config::default());
    assert!(env.calls().contains(&"del thunderbolt0:1".to_string()));
    // default: never added
    let env = tb_up_env();
    let mut m = Manager::new();
    m.tick(&env, &Config::default());
    env.with(|s| s.inbox.insert("thunderbolt0".into(), vec![(Ipv4Addr::new(169, 254, 77, 1), beacon("other-box", MAC_B, "ffeeddccbbaa99887766554433221100", true, false).encode())]));
    m.tick(&env, &Config::default());
    assert!(env.calls().iter().all(|c| !c.contains("10.77")));
}

#[test]
fn switched_off_in_the_settings_the_manager_touches_nothing_and_vanished_interfaces_are_forgotten() {
    let env = tb_up_env();
    let mut m = Manager::new();
    m.tick(&env, &Config { enabled: false, ..Config::default() });
    assert!(env.calls().is_empty() && m.zone().is_empty());
    m.tick(&env, &Config::default());
    assert_eq!(m.zone().len(), 1);
    env.with(|s| s.ifaces.retain(|i| i.name != "thunderbolt0"));
    assert!(m.tick(&env, &Config::default()).links.is_empty() && m.zone().is_empty());
}

// ---------------------------------------------------------------- the check list

fn texts(r: &checks::Report) -> Vec<(State, String)> {
    r.checks.iter().map(|c| (c.state, c.text.clone())).collect()
}

fn ready_env() -> FakeEnv {
    let env = FakeEnv::new();
    let ll = Ipv4Addr::new(169, 254, 5, 6);
    env.with(|s| {
        let mut i = iface("thunderbolt0", "thunderbolt-net", MAC_A, true, true);
        i.ipv4 = vec![ll];
        i.ipv6_ll = vec!["fe80::1%thunderbolt0".into()];
        s.ifaces = vec![i];
        s.tb = tb_view("none", vec![xdomain()]);
        s.snapshot = Some(Snapshot { at: 1000, links: vec![], peers: vec![Peer { iface: "thunderbolt0".into(), addr: Ipv4Addr::new(169, 254, 7, 7), beacon: beacon("other-box", MAC_B, "ffeeddccbbaa99887766554433221100", true, true), last_seen: 999 }] });
        s.hello = Ok(Hello { name: "other-box".into(), version: "0.1.0-test".into(), fingerprint: "AA:BB:CC:DD".into(), pairing_open: true });
    });
    env
}

#[test]
fn a_plain_pc_with_nothing_connected_fails_step_one_and_explains_the_hardware_limit() {
    let env = FakeEnv::new();
    env.with(|s| s.ifaces = vec![iface("eth0", "e1000e", MAC_A, true, true)]);
    let r = checks::run(&env, None);
    assert_eq!(r.checks[0].state, State::Fail);
    let t = &r.checks[0].text;
    assert!(t.contains("plain USB-C cable") && t.contains("both ports are USB hosts") && t.contains("Thunderbolt 3, Thunderbolt 4 or USB4") && t.contains("bridge cable"), "{t}");
    assert!(r.checks[1..].iter().all(|c| c.state == State::Wait && c.text.starts_with("Waiting for step")), "{:?}", texts(&r));
    assert!(r.last.contains("0 of 7") && r.last.contains("Cable and port"), "{}", r.last);
}

#[test]
fn the_steps_for_thunderbolt_turn_green_one_by_one_and_each_failure_says_what_to_do() {
    let env = FakeEnv::new();
    // controller present, driver not loaded
    env.with(|s| s.tb = TbView { controller_present: true, driver_loaded: false, ..TbView::default() });
    let r = checks::run(&env, None);
    assert_eq!(r.checks[0].state, State::Fail);
    assert!(r.checks[0].text.contains("modprobe thunderbolt") && r.checks[0].text.contains("CONFIG_USB4"));
    // driver loaded, nothing connected
    env.with(|s| s.tb = tb_view("user", vec![]));
    let r = checks::run(&env, None);
    assert_eq!((r.checks[0].state, r.checks[1].state), (State::Pass, State::Fail));
    assert!(r.checks[0].text.contains("Security level: user"));
    assert!(r.checks[1].text.contains("BOTH computers") && r.checks[1].text.contains("charges") && r.checks[1].text.contains("lightning"), "{}", r.checks[1].text);
    // a device waits for approval: the button is offered
    env.with(|s| s.tb = tb_view("user", vec![router(0)]));
    let r = checks::run(&env, None);
    assert_eq!(r.checks[1].state, State::Fail);
    assert_eq!(r.checks[1].action, Some(Action::Authorize("0-3".into())));
    assert!(r.checks[1].text.contains("Authorize") && r.checks[1].text.contains("approve each new device"));
    assert!(checks::authorize_pending(&env, &r).unwrap().contains("approved"));
    assert_eq!(checks::authorize_pending(&env, &checks::run(&env, None)).unwrap_err(), "nothing waits for approval");
    // the other computer is connected but thunderbolt0 is missing
    env.with(|s| s.tb = tb_view("none", vec![xdomain()]));
    let r = checks::run(&env, None);
    assert_eq!(r.checks[1].state, State::Pass);
    assert!(r.checks[1].text.contains("other-box") && r.checks[1].text.contains("20/20 Gbit/s over 2 lane(s)"), "{}", r.checks[1].text);
    assert_eq!(r.checks[2].state, State::Fail);
    assert!(r.checks[2].text.contains("thunderbolt_net") && r.checks[2].text.contains("CONFIG_USB4_NET"));
    // the interface exists but has no address yet
    env.with(|s| s.ifaces = vec![iface("thunderbolt0", "thunderbolt-net", MAC_A, true, true)]);
    let r = checks::run(&env, None);
    assert!(r.checks[2].state == State::Fail && r.checks[2].text.contains("no address yet"), "{}", r.checks[2].text);
    let r2 = {
        env.with(|s| s.ifaces[0].up = false);
        checks::run(&env, None)
    };
    assert!(r2.checks[2].text.contains("it is down"));
    // address up, nobody has sent a beacon
    env.with(|s| {
        s.ifaces[0].up = true;
        s.ifaces[0].ipv4 = vec![Ipv4Addr::new(169, 254, 5, 6)];
    });
    let r = checks::run(&env, None);
    assert_eq!(r.checks[2].state, State::Pass);
    assert!(r.checks[2].text.contains("169.254.5.6") && r.checks[2].text.contains("10 Gbit/s"), "{}", r.checks[2].text);
    assert_eq!(r.checks[3].state, State::Fail);
    assert!(r.checks[3].text.contains("No IYAKKI answered") && r.checks[3].text.contains("iyakki dev pair <address>"));
}

#[test]
fn beacon_developer_mode_pairing_and_speed_steps() {
    let env = ready_env();
    // peer found by the beacon (from the manager's state); developer mode refuses: off there
    env.with(|s| s.hello = Err("developer mode is off on this device".into()));
    let r = checks::run(&env, None);
    assert_eq!(r.checks[3].state, State::Pass);
    assert!(r.checks[3].text.contains("other-box") && r.checks[3].text.contains("169.254.7.7") && r.checks[3].text.contains("a pairing code is open"));
    assert_eq!(r.checks[4].state, State::Fail);
    assert!(r.checks[4].text.contains("switched off on the other computer") && r.checks[4].text.contains("press e"));
    // connection refused: firewall words
    env.with(|s| s.hello = Err("connection refused".into()));
    let r = checks::run(&env, None);
    assert!(r.checks[4].text.contains("7979") && r.checks[4].text.contains("direct-link zone"), "{}", r.checks[4].text);
    // developer mode answers, not paired
    env.with(|s| s.hello = Ok(Hello { name: "other-box".into(), version: "0.1.0-test".into(), fingerprint: "AA:BB:CC:DD".into(), pairing_open: false }));
    let r = checks::run(&env, None);
    assert_eq!(r.checks[4].state, State::Pass);
    assert_eq!((r.checks[5].state, r.checks[5].action.clone()), (State::Fail, Some(Action::Pair)));
    assert!(r.checks[5].text.contains("press p") && r.checks[5].text.contains("nobody on the cable can pair without someone at that screen"));
    env.with(|s| s.hello.as_mut().unwrap().pairing_open = true);
    assert!(checks::run(&env, None).checks[5].text.contains("A pairing code is open there"));
    // pairing needs the right code
    let r = checks::run(&env, None);
    assert_eq!(checks::pair(&env, &r, "").unwrap_err(), "type the code shown on the other computer");
    assert_eq!(checks::pair(&env, &r, "WRONG").unwrap_err(), "the code is wrong");
    assert!(env.with(|s| s.paired.is_none()));
    // the speed check refuses before pairing
    assert!(checks::speed_test(&env, &r).unwrap_err().contains("pair first"));
    assert!(checks::pair(&env, &r, " K7QFM-2XPRT ").unwrap().contains("Paired"));
    assert!(env.calls().iter().any(|c| c == "pair K7QFM-2XPRT AA:BB:CC:DD"), "the code and the fingerprint the other side showed go to the pairing: {:?}", env.calls());
    // paired: step 6 passes, step 7 waits for a measurement
    let r = checks::run(&env, None);
    assert_eq!(r.checks[5].state, State::Pass);
    assert_eq!((r.checks[6].state, r.checks[6].action.clone()), (State::Wait, Some(Action::Speed)));
    assert!(r.last.contains("6 of 7") && r.last.contains("speed check has not been run"));
    // measure: 64 MiB in 0.4 s is about 1.34 Gbit/s
    env.with(|s| s.speed = Ok(SpeedSample { bytes: 64 << 20, secs: 0.4 }));
    let sp = checks::speed_test(&env, &r).unwrap();
    assert!((sp.sample.gbit_per_s() - 1.342).abs() < 0.01, "{}", sp.sample.gbit_per_s());
    assert_eq!(sp.rtt_ms, Some(0.3));
    let r = checks::run(&env, Some(&sp));
    assert_eq!(r.checks[6].state, State::Pass);
    assert!(r.checks[6].text.contains("1.34 Gbit/s") && r.checks[6].text.contains("64 MiB") && r.checks[6].text.contains("delay 0.30 ms"), "{}", r.checks[6].text);
    assert_eq!(r.last, "All 7 steps passed.");
    // a speed run that moved nothing is an error, not 0 Gbit/s
    env.with(|s| s.speed = Ok(SpeedSample { bytes: 0, secs: 0.0 }));
    assert!(checks::speed_test(&env, &r).unwrap_err().contains("no data"));
    env.with(|s| s.speed = Err("connection reset".into()));
    assert!(checks::speed_test(&env, &r).is_err());
}

#[test]
fn the_speed_numbers_are_computed_safely() {
    assert_eq!(SpeedSample { bytes: 1_000_000_000, secs: 1.0 }.gbit_per_s(), 8.0);
    assert_eq!(SpeedSample { bytes: 100, secs: 0.0 }.gbit_per_s(), 0.0);
    assert_eq!(SpeedSample { bytes: 100, secs: -1.0 }.gbit_per_s(), 0.0);
    assert_eq!(SpeedSample { bytes: 100, secs: f64::NAN }.gbit_per_s(), 0.0);
    assert_eq!(SpeedSample { bytes: u64::MAX, secs: f64::INFINITY }.gbit_per_s(), 0.0);
    assert_eq!(SPEED_CHUNK * SPEED_CHUNKS, 64 << 20, "the speed check is 64 MiB");
    assert!(SPEED_CHUNK <= crate::devmode::api::SPEED_MAX_BODY, "one request never exceeds what the device accepts");
}

#[test]
fn a_peer_is_looked_for_actively_when_the_manager_has_none_and_other_links_peers_do_not_count() {
    let env = ready_env();
    let peer = env.with(|s| s.snapshot.take().unwrap().peers[0].clone());
    // no manager state: the short scan finds it
    env.with(|s| s.discovered = vec![peer.clone()]);
    assert_eq!(checks::run(&env, None).checks[3].state, State::Pass);
    // a peer seen on another interface is not the peer of this cable
    env.with(|s| s.discovered = vec![Peer { iface: "enx0".into(), ..peer }]);
    assert_eq!(checks::run(&env, None).checks[3].state, State::Fail);
}

#[test]
fn bridge_cables_and_gadgets_are_steps_one_and_two_too() {
    // a bridge cable with a link
    let env = FakeEnv::new();
    env.with(|s| s.ifaces = vec![iface("enx0011", "plusb", MAC_A, true, true)]);
    let r = checks::run(&env, None);
    assert!(r.checks[0].state == State::Pass && r.checks[0].text.contains("USB host-to-host bridge cable") && r.checks[0].text.contains("plusb"));
    assert_eq!(r.checks[1].state, State::Pass);
    assert_eq!(r.checks[2].state, State::Fail, "up, but no address yet");
    // the cable is plugged in on this side only
    env.with(|s| s.ifaces[0].carrier = false);
    let r = checks::run(&env, None);
    assert!(r.checks[1].state == State::Fail && r.checks[1].text.contains("no link"));
    // a computer with a USB device controller and nothing connected yet
    let env = FakeEnv::new();
    env.with(|s| s.udc = true);
    let r = checks::run(&env, None);
    assert!(r.checks[0].state == State::Pass && r.checks[0].text.contains("gadget on"));
    assert_eq!(r.checks[1].state, State::Fail);
}

#[test]
fn the_diagnostic_text_lists_everything_and_holds_no_secret() {
    let env = ready_env();
    env.with(|s| s.keys.insert("aa-bb".into(), "ee".repeat(32)));
    let r = checks::run(&env, None);
    let d = r.diagnostic(&env);
    assert!(d.contains("IYAKKI direct link report") && d.contains("dev-pc") && d.contains("last result:") && d.contains("thunderbolt0") && d.contains("security=none") && d.contains("[ok]"));
    assert!(!d.contains(&"ee".repeat(8)), "no key in the report");
    assert!(!d.contains("K7QFM"), "no pairing code in the report");
}

// ---------------------------------------------------------------- the gadget and the command line

#[derive(Default)]
struct RecFs {
    ops: std::sync::Mutex<Vec<String>>,
    fail_on: Option<&'static str>,
}

impl RecFs {
    fn rec(&self, s: String) -> Result<(), String> {
        if self.fail_on.is_some_and(|f| s.contains(f)) {
            return Err(format!("{s}: refused"));
        }
        self.ops.lock().unwrap().push(s);
        Ok(())
    }
}

impl gadget::Fs for RecFs {
    fn mkdir(&self, p: &str) -> Result<(), String> {
        self.rec(format!("mkdir {p}"))
    }
    fn write(&self, p: &str, v: &str) -> Result<(), String> {
        self.rec(format!("write {p} {v}"))
    }
    fn symlink(&self, to: &str, at: &str) -> Result<(), String> {
        self.rec(format!("symlink {at} -> {to}"))
    }
    fn rmdir(&self, p: &str) -> Result<(), String> {
        self.rec(format!("rmdir {p}"))
    }
    fn remove(&self, p: &str) -> Result<(), String> {
        self.rec(format!("rm {p}"))
    }
}

#[test]
fn the_gadget_plan_binds_last_stays_inside_its_folder_and_is_undone_on_failure() {
    let plan = gadget::plan_on("fe800000.usb", gadget::Function::Ncm, "box").unwrap();
    assert!(matches!(plan.last(), Some(gadget::Op::Write(p, v)) if p.ends_with("/UDC") && v == "fe800000.usb"), "bound to the controller last");
    let all: Vec<String> = plan.iter().map(|o| format!("{o:?}")).collect();
    assert!(all.iter().all(|s| s.contains(gadget::ROOT)), "{all:?}");
    assert!(all.iter().any(|s| s.contains("ncm.usb0")) && all.iter().any(|s| s.contains("host_addr")));
    let (h, d) = gadget::macs("box");
    assert_ne!(h, d);
    assert!(linklocal::parse_mac(&h).is_some() && mac(&h)[0] & 1 == 0 && mac(&h)[0] & 2 == 2, "a locally administered unicast address: {h}");
    assert_eq!(gadget::macs("box"), (h, d), "stable between runs");
    assert!(gadget::plan_on("", gadget::Function::Ecm, "b").is_err() && gadget::plan_on("a/../b", gadget::Function::Ecm, "b").is_err());
    let fs = RecFs::default();
    gadget::apply(&fs, &plan, true).unwrap();
    assert_eq!(fs.ops.lock().unwrap().len(), plan.len());
    // a step fails (no such function in this kernel): what was made is taken apart again
    let fs = RecFs { fail_on: Some("functions/ncm"), ..RecFs::default() };
    let e = gadget::apply(&fs, &plan, true).unwrap_err();
    assert!(e.contains("taken apart"), "{e}");
    assert!(fs.ops.lock().unwrap().iter().any(|o| o.starts_with("write") && o.ends_with("/UDC ")), "unbound first");
    // the real file system refuses anything outside the folder
    assert!(gadget::guard_for_test("/etc/passwd").is_err() && gadget::guard_for_test("/sys/kernel/config/usb_gadget/iyakki/../x").is_err());
    // off: every step is tried even when some are missing
    let fs = RecFs { fail_on: Some("ecm"), ..RecFs::default() };
    gadget::apply(&fs, &gadget::plan_off(), false).unwrap();
    assert!(fs.ops.lock().unwrap().last().unwrap().starts_with("rmdir"));
}

#[test]
fn the_command_line_shows_the_steps_pairs_with_a_code_and_changes_the_link_settings() {
    let env = ready_env();
    env.with(|s| s.hello = Ok(Hello { name: "other-box".into(), version: "0.1.0-test".into(), fingerprint: "AA:BB:CC:DD".into(), pairing_open: true }));
    let run = |args: &[&str], ask: &'static str| {
        let mut out = Vec::new();
        let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let r = cli::run_with(&env, &a, &mut |l| out.push(l.to_string()), &|_| ask.to_string());
        (r, out.join("\n"))
    };
    let (r, o) = run(&[], "");
    assert!(r.is_ok() && o.contains("[ ok ] 1  Cable") && o.contains("[FAIL] 6  This computer is paired") && o.contains("Last result: 5 of 7"), "{o}");
    // the code is asked for after the fingerprint is shown
    let (r, o) = run(&["pair"], "K7QFM-2XPRT");
    assert!(r.is_ok(), "{r:?} {o}");
    assert!(o.contains("certificate fingerprint: AA:BB:CC:DD") && o.contains("Paired with other-box"), "{o}");
    assert!(run(&["pair", "--code", "nope"], "").0.is_err() || env.with(|s| s.paired.is_some()));
    env.with(|s| s.speed = Ok(SpeedSample { bytes: 64 << 20, secs: 0.2 }));
    let (r, o) = run(&["speed"], "");
    assert!(r.is_ok() && o.contains("Gbit/s") && o.contains("64 MiB"), "{r:?} {o}");
    let (r, o) = run(&["link", "--fixed-pair", "on"], "");
    assert!(r.is_ok() && o.contains("fixed private pair (10.77.0.1 / 10.77.0.2): on") && o.contains("direct links: off"), "{o}");
    assert!(env.config().fixed_pair && !env.config().allow_more);
    assert!(run(&["link", "--allow-more", "maybe"], "").0.is_err());
    assert!(run(&["gadget"], "").0.unwrap_err().contains("usage"));
    assert!(run(&["bogus"], "").0.unwrap_err().contains("usage"));
    // the gadget needs a controller
    let e = cli::gadget_run(true, &tmp("udc-none"), &RecFs::default(), &mut |_| {}).unwrap_err();
    assert!(e.contains("ordinary PC cannot do this"), "{e}");
    let udc = tmp("udc");
    put(&udc, "fe800000.usb/uevent", "");
    let fs = RecFs::default();
    let mut said = Vec::new();
    cli::gadget_run(true, &udc, &fs, &mut |l| said.push(l.to_string())).unwrap();
    assert!(said[0].contains("fe800000.usb") && !fs.ops.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(udc);
}
