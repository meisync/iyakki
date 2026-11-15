//! The direct-link manager on the REAL system layer, with a real cable (ignored by default): the ioctls, the raw ARP socket, the sysctl and the beacon socket of
//! `directlink::real::RealEnv` against the kernel, two network namespaces joined by a veth pair named `thunderbolt0` (so `classify` takes it for a Thunderbolt cable).
//!
//! Needs root, `ip`, `nsenter`, `unshare`, `python3`; run it only through `dist/linux-host/kernel-tests.sh`, which starts it in a throw-away network namespace (the machine's own
//! network is never touched). Computer A is this namespace with the real manager; computer B is a second namespace that holds the address A would take first, sends a beacon and
//! listens for A's. What it checks: the manager switches the interface up, forbids forwarding on it, probes with real ARP, meets the conflict (B answers for the first candidate)
//! and takes the next candidate, announces it, sends its beacon on the cable only, reads B's beacon into the peer table, and leaves an ordinary interface (not a direct link)
//! alone. Row: F-NET-14.

#![cfg(target_os = "linux")]

use iyakki_agent::directlink::beacon::Beacon;
use iyakki_agent::directlink::linklocal::{candidate, parse_mac};
use iyakki_agent::directlink::manager::Manager;
use iyakki_agent::directlink::real::RealEnv;
use iyakki_agent::directlink::{Config, Transport};
use std::process::{Child, Command, Stdio};

fn sh(cmd: &str) -> (bool, String) {
    let o = Command::new("sh").args(["-c", &format!("export PATH=$PATH:/usr/sbin:/sbin; {cmd}")]).output().unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

fn spawn_ns() -> Child {
    let c = Command::new("unshare").args(["-n", "sleep", "300"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    c
}

/// B: listens for beacons on UDP 7980 (writes each datagram as a hex line) and sends one beacon a second from its own address.
const PEER: &str = r#"
import socket, sys, threading, time
iface, hexpayload, out = sys.argv[1], sys.argv[2], sys.argv[3]
payload = bytes.fromhex(hexpayload)
def listen():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(('0.0.0.0', 7980))
    while True:
        d, a = s.recvfrom(1024)
        with open(out, 'a') as f: f.write(a[0] + ' ' + d.hex() + '\n')
threading.Thread(target=listen, daemon=True).start()
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
s.setsockopt(socket.SOL_SOCKET, socket.SO_BINDTODEVICE, iface.encode())
for _ in range(120):
    try: s.sendto(payload, ('255.255.255.255', 7980))
    except OSError: pass
    time.sleep(1)
"#;

#[test]
#[ignore]
fn the_link_manager_brings_a_real_cable_up_meets_a_real_conflict_and_hears_the_other_computer() {
    // SAFETY: getuid has no preconditions.
    let root = unsafe { libc::getuid() } == 0 && std::env::var("IYAKKI_IN_SANDBOX").as_deref() == Ok("1");
    if !(root && sh("command -v python3 >/dev/null && command -v nsenter >/dev/null && command -v unshare >/dev/null && command -v ip >/dev/null").0) {
        eprintln!("skipped: needs root, python3, ip, nsenter and unshare (run it through dist/linux-host/kernel-tests.sh)");
        return;
    }
    sh("ip link set lo up");
    let mut b = spawn_ns();
    let bp = b.id();
    let (mac_a, mac_b) = ("02:11:22:33:44:55", "02:aa:bb:cc:dd:ee");
    // the cable: A's end is called thunderbolt0 and left DOWN (the manager must switch it up); an ordinary interface next to it must stay as it is
    let (ok, out) = sh(&format!(
        "ip link add thunderbolt0 type veth peer name peer0 && ip link set peer0 netns {bp} && ip link set thunderbolt0 address {mac_a} && \
         nsenter -t {bp} -n sh -c 'ip link set lo up && ip link set peer0 address {mac_b} && ip link set peer0 up' && \
         ip link add plain0 type veth peer name plain1 && ip link set plain1 up"
    ));
    assert!(ok, "{out}");
    let ma = parse_mac(mac_a).unwrap();
    let (first, second) = (candidate(&ma, 0), candidate(&ma, 1));
    // B already has the address A would take first
    let (ok, out) = sh(&format!("nsenter -t {bp} -n ip addr add {first}/16 dev peer0"));
    assert!(ok, "{out}");
    let dir = std::path::PathBuf::from(format!("/tmp/iyakki-dl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let b_beacon = Beacon { host: "peer-computer".into(), version: "0.0.0".into(), role: "target".into(), dev_on: true, pairing_open: false, mac: mac_b.into(), id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(), port: 7979, ip6: None };
    let hex: String = b_beacon.encode().iter().map(|x| format!("{x:02x}")).collect();
    let got = dir.join("heard.txt");
    let mut peer = Command::new("nsenter").args(["-t", &bp.to_string(), "-n", "python3", "-c", PEER, "peer0", &hex, got.to_str().unwrap()]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();

    let env = RealEnv::new(&dir.join("data"), &dir.join("home"));
    let cfg = Config::default();
    let mut m = Manager::new();
    let mut snap = m.tick(&env, &cfg);
    let end = std::time::Instant::now() + std::time::Duration::from_secs(90);
    while std::time::Instant::now() < end {
        snap = m.tick(&env, &cfg);
        if snap.links.iter().any(|l| l.link_local.is_some()) && !snap.peers.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    // the cable is a managed direct link with the second candidate (the first was taken by B and found by a real ARP probe)
    let link = snap.links.iter().find(|l| l.name == "thunderbolt0").unwrap_or_else(|| panic!("no managed link: {snap:?}"));
    assert_eq!(link.transport, Transport::Thunderbolt);
    assert!(link.carrier, "the cable has a link once both ends are up");
    assert_eq!(link.link_local, Some(second), "the first candidate {first} is B's: the manager must have moved on to {second}");
    assert_eq!(m.zone(), vec!["thunderbolt0".to_string()], "only the cable is a direct link");
    let addrs = sh("ip -4 -o addr show dev thunderbolt0").1;
    assert!(addrs.contains(&format!("{second}/16")), "the kernel has the address: {addrs}");
    assert_eq!(std::fs::read_to_string("/proc/sys/net/ipv4/conf/thunderbolt0/forwarding").unwrap().trim(), "0", "no forwarding on the cable");
    // the ordinary interface was left alone: not up by the manager, no address, not in the zone
    let plain = sh("ip -o link show plain0").1;
    assert!(plain.contains("state DOWN") || plain.contains("NO-CARRIER") && !plain.contains(",UP,"), "an ordinary interface stays as it was: {plain}");
    assert!(sh("ip -4 -o addr show dev plain0").1.trim().is_empty());
    // B's beacon is in the peer table, with B's address as the sender
    let peer_row = snap.peers.iter().find(|p| p.beacon.id == b_beacon.id).unwrap_or_else(|| panic!("B's beacon was not heard: {snap:?}"));
    assert_eq!((peer_row.iface.as_str(), peer_row.addr, peer_row.beacon.host.as_str()), ("thunderbolt0", first, "peer-computer"));
    // A's own beacon reached B on the cable, and parses
    let heard = std::fs::read_to_string(&got).unwrap_or_default();
    let line = heard.lines().find(|l| !l.contains(&hex)).unwrap_or_else(|| panic!("B heard nothing from A: {heard:?}"));
    let (src, h) = line.split_once(' ').unwrap();
    assert_eq!(src, second.to_string(), "A's beacon comes from its link-local address");
    let bytes: Vec<u8> = (0..h.len() / 2).map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap()).collect();
    let a_beacon = Beacon::parse(&bytes).expect("A's beacon parses");
    assert_eq!(a_beacon.mac, mac_a);
    assert_ne!(a_beacon.id, b_beacon.id);
    let _ = peer.kill();
    let _ = peer.wait();
    let _ = b.kill();
    let _ = b.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
