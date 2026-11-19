//! Real WireGuard, real nf_tables and real routing in two private network namespaces (ignored by default).
//!
//! Needs root (`wireguard` and nf_tables are root-only in a fresh namespace), the `ip`, `ping`, `openssl` and `nsenter` programs and the kernel's `wireguard`
//! module. Run it only through `dist/linux-host/kernel-tests.sh`, which starts the test program in a throw-away network namespace (`unshare -n`) so the
//! machine's own network and firewall are never touched. The test makes a second namespace B itself (`unshare -n sleep`), joins A (the test's own
//! namespace, the "client") and B (the "server") with a veth pair and runs IYAKKI's own system layer (`vpn_sys::OsSys`: generic-netlink WireGuard
//! messages, the `ip` steps, the guard and DNS-capture tables) on each side. Rows: F-VPN-03 (WireGuard), F-VPN-14 (guard and DNS-capture tables with real traffic).

use iyakki_agent::vpn_sys::{OsSys, Sys};
use iyakki_vpn::wgnl::{DeviceSet, PeerSet};
use iyakki_vpn::plan::{dns_capture, fail_closed_guard, kill_switch, FailClosedInput, GuardBase, KillSwitchInput};
use iyakki_vpn::profile::PublicKey;
use iyakki_vpn::{Cidr, Secret};
use std::process::{Child, Command, Stdio};

fn sh(cmd: &str) -> String {
    let o = Command::new("sh").args(["-c", cmd]).output().expect("sh");
    assert!(o.status.success(), "{cmd}: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn have(tool: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {tool} >/dev/null")]).status().is_ok_and(|s| s.success())
}

fn is_root() -> bool {
    // SAFETY: getuid has no preconditions.
    let root = unsafe { libc::getuid() == 0 };
    root && std::env::var("IYAKKI_IN_SANDBOX").as_deref() == Ok("1")
}

/// A WireGuard key pair made with openssl (X25519): base64 of the last 32 bytes of the DER forms, private then public.
fn keypair() -> (String, String) {
    let dir = std::env::temp_dir().join(format!("iyakki-vpnk-{}-{}", std::process::id(), rand_tag()));
    std::fs::create_dir_all(&dir).unwrap();
    sh(&format!("cd {d} && openssl genpkey -algorithm X25519 -out k.pem 2>/dev/null && openssl pkey -in k.pem -outform DER | tail -c 32 | base64 > priv && openssl pkey -in k.pem -pubout -outform DER | tail -c 32 | base64 > pub", d = dir.display()));
    let p = std::fs::read_to_string(dir.join("priv")).unwrap().trim().to_string();
    let q = std::fs::read_to_string(dir.join("pub")).unwrap().trim().to_string();
    let _ = std::fs::remove_dir_all(&dir);
    (p, q)
}

fn rand_tag() -> usize {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos() as usize)
}

struct Ns {
    child: Child,
    pid: u32,
}

impl Ns {
    fn new() -> Ns {
        let child = Command::new("unshare").args(["-n", "sleep", "300"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("unshare -n");
        let pid = child.id();
        // wait until the child has really entered its namespace (its net namespace differs from ours)
        let mine = std::fs::read_link("/proc/self/ns/net").unwrap();
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(30) {
            if std::fs::read_link(format!("/proc/{pid}/ns/net")).is_ok_and(|l| l != mine) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Ns { child, pid }
    }
    fn run(&self, cmd: &str) -> String {
        sh(&format!("nsenter -t {} -n sh -c '{}'", self.pid, cmd.replace('\'', "'\\''")))
    }
    /// Runs `f` on a thread that has joined this namespace (a network namespace belongs to the thread, so IYAKKI's netlink sockets made inside land here).
    fn on<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> T {
        let path = std::ffi::CString::new(format!("/proc/{}/ns/net", self.pid)).unwrap();
        std::thread::spawn(move || {
            // SAFETY: open and setns on a path made above; the fd is closed after use.
            unsafe {
                let fd = libc::open(path.as_ptr(), libc::O_RDONLY);
                assert!(fd >= 0, "open the namespace");
                assert_eq!(libc::setns(fd, libc::CLONE_NEWNET), 0, "setns");
                libc::close(fd);
            }
            f()
        })
        .join()
        .expect("the thread in the namespace")
    }
}

impl Drop for Ns {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ready() -> bool {
    if !(is_root() && have("ip") && have("ping") && have("openssl") && have("nsenter") && have("unshare")) {
        eprintln!("skipped: needs root, ip, ping, openssl, nsenter and unshare (run it through dist/linux-host/kernel-tests.sh)");
        return false;
    }
    if !Command::new("sh").args(["-c", "ip link add m80probe type wireguard && ip link del m80probe"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipped: the kernel has no wireguard module");
        return false;
    }
    true
}

#[test]
#[ignore]
fn a_wireguard_tunnel_between_two_namespaces_is_made_by_the_system_layer_carries_traffic_and_is_taken_down() {
    if !ready() {
        return;
    }
    sh("ip link set lo up");
    let b = Ns::new();
    b.run("ip link set lo up");
    // the "internet" between the two: a veth pair
    sh("ip link add vA type veth peer name vB");
    sh(&format!("ip link set vB netns {}", b.pid));
    sh("ip addr add 10.77.0.1/24 dev vA && ip link set vA up");
    b.run("ip addr add 10.77.0.2/24 dev vB && ip link set vB up");
    sh("ping -c1 -W2 10.77.0.2 >/dev/null");

    let (a_priv, a_pub) = keypair();
    let (b_priv, b_pub) = keypair();
    let a_priv_s = Secret::from_key_b64(&a_priv).unwrap();
    let b_pub_k = PublicKey::parse(&b_pub).unwrap();

    // server B: device wg1, listen 51820, peer A allowed 10.99.0.1/32
    b.run("ip link add wg1 type wireguard");
    {
        let (sk, pk) = (b_priv.clone(), a_pub.clone());
        b.on(move || {
            let sys = OsSys::default();
            let (sk, pk) = (Secret::from_key_b64(&sk).unwrap(), PublicKey::parse(&pk).unwrap());
            let allowed = [Cidr::parse("10.99.0.1/32").unwrap()];
            let mut peer = PeerSet::new(&pk, &allowed);
            peer.replace_allowed_ips = true;
            sys.wg_configure(&DeviceSet { ifname: "wg1", private_key: Some(&sk), listen_port: Some(51820), fwmark: None, replace_peers: true, peers: vec![peer] }).expect("configure the server device");
        });
    }
    b.run("ip addr add 10.99.0.2/32 dev wg1 && ip link set wg1 up && ip route add 10.99.0.1/32 dev wg1");

    // client A: the same steps IYAKKI's manager takes, through the real system layer
    let sys = OsSys::default();
    assert!(!sys.link_exists("wg0"));
    sys.ip(&["link".into(), "add".into(), "dev".into(), "wg0".into(), "type".into(), "wireguard".into()]).expect("ip link add");
    assert!(sys.link_exists("wg0"));
    let allowed = [Cidr::parse("10.99.0.2/32").unwrap()];
    let mut peer = PeerSet::new(&b_pub_k, &allowed);
    peer.endpoint = Some("10.77.0.2:51820".parse().unwrap());
    peer.keepalive = Some(5);
    peer.replace_allowed_ips = true;
    sys.wg_configure(&DeviceSet { ifname: "wg0", private_key: Some(&a_priv_s), listen_port: None, fwmark: None, replace_peers: true, peers: vec![peer] }).expect("configure the client device");
    for step in [&["addr", "add", "10.99.0.1/32", "dev", "wg0"][..], &["link", "set", "wg0", "up"], &["route", "add", "10.99.0.2/32", "dev", "wg0"]] {
        sys.ip(&step.iter().map(|s| s.to_string()).collect::<Vec<_>>()).expect("ip step");
    }
    // the device reads back what was written (generic netlink, both directions) and the kernel agrees with `ip`
    let st = sys.wg_status("wg0").expect("status");
    assert_eq!(st.public_key, Some(PublicKey::parse(&a_pub).unwrap()), "the public key the kernel derived from our private key");
    assert_eq!(st.peers.len(), 1);
    assert_eq!(st.peers[0].public_key, b_pub_k);
    assert_eq!(st.peers[0].endpoint, Some("10.77.0.2:51820".parse().unwrap()));
    assert_eq!(st.peers[0].keepalive, 5);
    assert_eq!(st.peers[0].allowed_ips, vec![Cidr::parse("10.99.0.2/32").unwrap()]);
    let details = sh("ip -d link show wg0");
    assert!(details.contains("wireguard"), "{details}");

    // traffic through the tunnel: the handshake happens, the bytes are counted, and the peer answers
    sh("ping -c3 -W3 -I wg0 10.99.0.2");
    let st = sys.wg_status("wg0").unwrap();
    assert!(st.peers[0].last_handshake.is_some_and(|t| t > 1_700_000_000), "a handshake took place: {:?}", st.peers[0].last_handshake);
    assert!(st.peers[0].rx_bytes > 0 && st.peers[0].tx_bytes > 0, "bytes counted: rx {} tx {}", st.peers[0].rx_bytes, st.peers[0].tx_bytes);
    let server = b.on(|| OsSys::default().wg_status("wg1").expect("server status"));
    assert!(server.peers[0].last_handshake.is_some() && server.peers[0].rx_bytes > 0, "the server saw the client: {server:?}");

    // take it down: removing the peer and the device leaves nothing
    sys.wg_configure(&DeviceSet { ifname: "wg0", private_key: None, listen_port: None, fwmark: None, replace_peers: true, peers: vec![] }).expect("remove the peers");
    assert!(sys.wg_status("wg0").unwrap().peers.is_empty());
    sys.ip(&["link".into(), "del".into(), "dev".into(), "wg0".into()]).expect("ip link del");
    assert!(!sys.link_exists("wg0"));
    assert!(sys.wg_status("wg0").is_err(), "a device that is gone answers with an error, not stale data");
}

fn udp_to(dest: &str, port: u16, wait: u64) -> bool {
    // true when a datagram sent to dest:port is answered or refused by that host within `wait` seconds is NOT what is asked: it asks "did the datagram leave the machine": the
    // route answers with an ICMP port-unreachable (connect() on a UDP socket reports it as a refusal on the next send/recv)
    let py = format!(
        "import socket,sys\ns=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.settimeout({wait}); s.connect(('{dest}',{port})); s.send(b'x')\ntry:\n    s.recv(10)\nexcept ConnectionRefusedError:\n    print('refused')\nexcept socket.timeout:\n    print('silent')\nexcept OSError as e:\n    print('error', e.errno)"
    );
    let o = Command::new("python3").args(["-c", &py]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).trim() == "refused"
}

fn ping_ok(dest: &str) -> bool {
    Command::new("ping").args(["-c1", "-W1", dest]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

#[test]
#[ignore]
fn the_kill_switch_the_fail_closed_guard_and_the_dns_capture_work_on_real_traffic() {
    if !ready() {
        return;
    }
    sh("ip link set lo up");
    let b = Ns::new();
    b.run("ip link set lo up");
    sh("ip link add vA type veth peer name vB");
    sh(&format!("ip link set vB netns {}", b.pid));
    sh("ip addr add 10.77.0.1/24 dev vA && ip link set vA up");
    b.run("ip addr add 10.77.0.2/24 dev vB && ip link set vB up");
    // two "internet" hosts behind B, reached through it
    b.run("ip addr add 198.51.100.7/32 dev lo && ip addr add 203.0.113.9/32 dev lo && sysctl -qw net.ipv4.conf.all.rp_filter=0");
    sh("ip route add 198.51.100.7/32 via 10.77.0.2 && ip route add 203.0.113.9/32 via 10.77.0.2");
    assert!(ping_ok("10.77.0.2") && ping_ok("198.51.100.7") && ping_ok("203.0.113.9"), "everything is reachable before any guard");

    let sys = OsSys::default();
    // what the manager reads about the connected network: a default route through B, parsed from the real `ip` output
    assert!(sys.net_info().is_err(), "no default route yet: the manager is told to connect to a network first");
    sh("ip route add default via 10.77.0.2 dev vA");
    let ni = sys.net_info().expect("net_info with a default route");
    assert_eq!((ni.physical_if.as_str(), ni.gateway4), ("vA", Some("10.77.0.2".parse().unwrap())), "{ni:?}");
    assert!(ni.lan.contains(&Cidr::parse("10.77.0.1/24").unwrap()) || ni.lan.iter().any(|c| c.to_string().starts_with("10.77.0.")), "the connected subnet: {ni:?}");
    let phys = vec!["vA".to_string()];
    let endpoints = [("10.77.0.2".parse().unwrap(), 51820u16, iyakki_vpn::plan::Proto::Udp)];
    let lan = [Cidr::parse("10.77.0.0/24").unwrap()];
    let base = || GuardBase { ifname: "wg0", physical_ifs: &phys, endpoints: &endpoints, lan: &lan, cluster: &[], cluster_if_prefixes: &["cni"], dev_port: 8443 };

    // FULL mode kill switch with a network prefix for the local network: the LAN works, everything else is dropped, until the guard is removed
    let ks = kill_switch(&KillSwitchInput { base: base(), block_dns_outside: false }).unwrap();
    assert!(!sys.guard_present());
    sys.install_guard(&ks).expect("the kill switch loads into the real kernel");
    assert!(sys.guard_present());
    assert!(ping_ok("10.77.0.2"), "the local network (a /24 prefix rule) keeps working under the kill switch");
    assert!(!ping_ok("198.51.100.7") && !ping_ok("203.0.113.9"), "traffic that is neither in the tunnel nor local is dropped");
    sys.remove_guard();
    assert!(!sys.guard_present());
    assert!(ping_ok("198.51.100.7"), "the guard is gone: traffic flows again");

    // HYBRID fail-closed guard: only the matched destinations (a prefix) are dropped, the rest is untouched
    let nets = [(Cidr::parse("198.51.100.0/24").unwrap(), true)];
    let fc = fail_closed_guard(&FailClosedInput { base: base(), nets: &nets }).unwrap();
    sys.install_guard(&fc).expect("the fail-closed guard with a network prefix loads into the real kernel");
    assert!(!ping_ok("198.51.100.7"), "a matched destination is dropped while the tunnel is down");
    assert!(ping_ok("203.0.113.9") && ping_ok("10.77.0.2"), "everything else is untouched");
    // a learned address joins the set while the guard is in place, and leaves it again
    sys.guard_set("203.0.113.9".parse().unwrap(), Some(60), true).expect("add a learned address");
    assert!(!ping_ok("203.0.113.9"), "an address learned from DNS is dropped too");
    sys.guard_set("203.0.113.9".parse().unwrap(), None, false).expect("remove it");
    assert!(ping_ok("203.0.113.9"), "and is let through again once it leaves the set");
    sys.remove_guard();
    assert!(ping_ok("198.51.100.7"));

    // DNS capture: a query to a resolver far away is redirected to the local forwarder's port
    assert!(udp_to("198.51.100.7", 53, 2), "before the capture the datagram reaches the far host, which refuses it (nothing listens there)");
    let spec = dns_capture(5353).unwrap();
    sys.install_dns_capture(&spec).expect("the DNS-redirect table loads into the real kernel");
    let l = std::net::UdpSocket::bind("127.0.0.1:5353").unwrap();
    l.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
    let sender = std::thread::spawn(|| {
        let s = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        s.send_to(b"query", "198.51.100.7:53").unwrap();
    });
    let mut buf = [0u8; 16];
    let got = l.recv_from(&mut buf);
    sender.join().unwrap();
    assert!(got.is_ok_and(|(n, _)| &buf[..n] == b"query"), "the query to port 53 of another host arrived at the local forwarder port");
    sys.remove_dns_capture();
    assert!(udp_to("198.51.100.7", 53, 2), "after removal the datagram goes to the far host again");
}

#[test]
#[ignore]
fn the_vpn_manager_connects_a_full_tunnel_over_the_real_system_layer_keeps_a_kill_switch_and_undoes_everything() {
    use iyakki_agent::gitkeys::{AuditFn, MemStore};
    use iyakki_agent::vpn_mgr::{Manager, State, IFNAME};
    use iyakki_vpn::profile::Settings;
    use std::sync::Arc;
    if !ready() {
        return;
    }
    sh("ip link set lo up");
    let dir = std::path::PathBuf::from(format!("/tmp/iyakki-vpnmgr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (resolv, backup) = (dir.join("resolv.conf"), dir.join("resolv.backup"));
    std::fs::write(&resolv, "nameserver 192.0.2.53\n").unwrap();
    let b = Ns::new();
    b.run("ip link set lo up");
    sh("ip link add vA type veth peer name vB");
    sh(&format!("ip link set vB netns {}", b.pid));
    sh("ip addr add 10.77.0.1/24 dev vA && ip link set vA up && ip route add default via 10.77.0.2");
    b.run("ip addr add 10.77.0.2/24 dev vB && ip link set vB up && ip addr add 203.0.113.50/32 dev lo");
    let (a_priv, a_pub) = keypair();
    let (b_priv, b_pub) = keypair();
    // the server: wg1 with the tunnel address 10.8.0.1, the client's key allowed as 10.8.0.2/32
    b.run("ip link add wg1 type wireguard");
    {
        let (sk, pk) = (b_priv.clone(), a_pub.clone());
        b.on(move || {
            let sys = OsSys::default();
            let (sk, pk) = (Secret::from_key_b64(&sk).unwrap(), PublicKey::parse(&pk).unwrap());
            let allowed = [Cidr::parse("10.8.0.2/32").unwrap()];
            let mut peer = PeerSet::new(&pk, &allowed);
            peer.replace_allowed_ips = true;
            sys.wg_configure(&DeviceSet { ifname: "wg1", private_key: Some(&sk), listen_port: Some(51820), fwmark: None, replace_peers: true, peers: vec![peer] }).expect("configure the server device");
        });
    }
    b.run("ip addr add 10.8.0.1/24 dev wg1 && ip link set wg1 up");
    let conf = format!("[Interface]\nPrivateKey = {a_priv}\nAddress = 10.8.0.2/24\nDNS = 10.8.0.1\n\n[Peer]\nPublicKey = {b_pub}\nEndpoint = 10.77.0.2:51820\nAllowedIPs = 0.0.0.0/0\nPersistentKeepalive = 5\n");
    let sys = Arc::new(OsSys { resolv_path: resolv.display().to_string(), backup_path: backup.display().to_string() });
    let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let l2 = log.clone();
    let audit: AuditFn = Arc::new(move |a, d| l2.lock().unwrap().push(format!("{a} {d}")));
    let vault = Arc::new(MemStore::default());
    let mgr = Manager::new(&dir.join("vpn"), vault, sys.clone(), audit);
    let id = mgr.import("Office", conf.as_bytes()).expect("import").profile.id;
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    mgr.set_settings(&id, Settings { kill_switch: true, ..Settings::full() }, now()).expect("settings");
    assert!(!sh("ip route get 203.0.113.50").contains(IFNAME), "before connecting the address is reached over the cable");
    mgr.set_enabled(&id, true, now()).expect("enable");
    let end = std::time::Instant::now() + std::time::Duration::from_secs(40);
    while std::time::Instant::now() < end && !matches!(mgr.status(&id).state, State::Connected { .. }) {
        mgr.tick(now());
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let st = mgr.status(&id);
    assert!(matches!(st.state, State::Connected { .. }), "the manager did not reach Connected: {st:?}\n{:?}", log.lock().unwrap());
    // the tunnel is the way out: routes, address, DNS, guard
    let route = sh("ip route get 203.0.113.50");
    assert!(route.contains(IFNAME), "everything goes through the tunnel now: {route}");
    assert!(sh("ip -4 -o addr show").contains("10.8.0.2/24") || sh("ip -4 -o addr show").contains("10.8.0.2/"), "tunnel address");
    assert!(sh("ip route get 10.77.0.2").contains("dev vA"), "the server itself is still reached over the cable (pinned endpoint route)");
    let now_resolv = std::fs::read_to_string(&resolv).unwrap();
    assert!(now_resolv.contains("nameserver 10.8.0.1") && !now_resolv.contains("192.0.2.53"), "the VPN's resolver is in place: {now_resolv}");
    assert!(std::fs::read_to_string(&backup).unwrap().contains("192.0.2.53"), "the original resolver list is kept");
    assert!(sys.guard_present() && st.guard, "the kill switch table is in place: {st:?}");
    assert!(ping_ok("10.8.0.1"), "the server's tunnel address answers");
    assert!(ping_ok("203.0.113.50"), "an address behind the server answers through the tunnel");
    let server = b.on(|| OsSys::default().wg_status("wg1").expect("server status"));
    assert!(server.peers[0].last_handshake.is_some() && server.peers[0].rx_bytes > 0, "the server saw the client: {server:?}");
    mgr.tick(now());
    assert!(mgr.status(&id).rx > 0 && mgr.status(&id).tx > 0, "the manager counts the bytes: {:?}", mgr.status(&id));
    // switched off: everything it put on the system is gone, the original state is back
    mgr.set_enabled(&id, false, now()).expect("disable");
    assert!(!sys.link_exists(IFNAME), "the tunnel interface is gone");
    assert!(!sys.guard_present(), "the guard goes with the profile when it is switched off");
    assert_eq!(std::fs::read_to_string(&resolv).unwrap().trim(), "nameserver 192.0.2.53", "the resolver list is back");
    assert!(!sh("ip route get 203.0.113.50").contains(IFNAME), "the default way out is back");
    assert!(matches!(mgr.status(&id).state, State::Stopped), "{:?}", mgr.status(&id));
    // deleting the profile leaves nothing in the vault or on the system
    mgr.remove(&id).expect("remove");
    assert!(!sys.link_exists(IFNAME));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore]
fn a_live_wireguard_tunnel_counts_as_the_local_network_for_the_real_firewall_table_only_when_the_setting_says_so() {
    use iyakki_fw::model::{Config, Resolved};
    use iyakki_fw::nl::NfSocket;
    use iyakki_fw::ruleset::{build, Addr, Ctx};
    if !ready() {
        return;
    }
    struct Wg0(std::process::Command);
    impl Drop for Wg0 {
        fn drop(&mut self) {
            let _ = self.0.status();
        }
    }
    let mut del = Command::new("sh");
    del.args(["-c", "ip link del wg0 2>/dev/null; nft delete table inet iyakki_fw 2>/dev/null"]).stdout(Stdio::null()).stderr(Stdio::null());
    let _cleanup = Wg0(del);
    sh("ip link set lo up");
    let b = Ns::new();
    b.run("ip link set lo up");
    sh("ip link add vA type veth peer name vB");
    sh(&format!("ip link set vB netns {}", b.pid));
    sh("ip addr add 10.77.0.1/24 dev vA && ip link set vA up");
    b.run("ip addr add 10.77.0.2/24 dev vB && ip link set vB up");
    let (a_priv, a_pub) = keypair();
    let (b_priv, b_pub) = keypair();
    // the tunnel's addresses are in 100.64.0.0/10 (what many VPNs hand out): not one of the ranges the firewall calls "the local network" by itself
    b.run("ip link add wg1 type wireguard");
    {
        let (sk, pk) = (b_priv.clone(), a_pub.clone());
        b.on(move || {
            let sys = OsSys::default();
            let (sk, pk) = (Secret::from_key_b64(&sk).unwrap(), PublicKey::parse(&pk).unwrap());
            let allowed = [Cidr::parse("100.64.0.2/32").unwrap()];
            let mut peer = PeerSet::new(&pk, &allowed);
            peer.replace_allowed_ips = true;
            sys.wg_configure(&DeviceSet { ifname: "wg1", private_key: Some(&sk), listen_port: Some(51820), fwmark: None, replace_peers: true, peers: vec![peer] }).expect("server device");
        });
    }
    b.run("ip addr add 100.64.0.1/32 dev wg1 && ip link set wg1 up && ip route add 100.64.0.2/32 dev wg1");
    let sys = OsSys::default();
    sys.ip(&["link".into(), "add".into(), "dev".into(), "wg0".into(), "type".into(), "wireguard".into()]).unwrap();
    let a_priv_s = Secret::from_key_b64(&a_priv).unwrap();
    let b_pub_k = PublicKey::parse(&b_pub).unwrap();
    let allowed = [Cidr::parse("100.64.0.1/32").unwrap()];
    let mut peer = PeerSet::new(&b_pub_k, &allowed);
    peer.endpoint = Some("10.77.0.2:51820".parse().unwrap());
    peer.keepalive = Some(2);
    peer.replace_allowed_ips = true;
    sys.wg_configure(&DeviceSet { ifname: "wg0", private_key: Some(&a_priv_s), listen_port: None, fwmark: None, replace_peers: true, peers: vec![peer] }).unwrap();
    for step in [&["addr", "add", "100.64.0.2/32", "dev", "wg0"][..], &["link", "set", "wg0", "up"], &["route", "add", "100.64.0.1/32", "dev", "wg0"]] {
        sys.ip(&step.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
    }
    sh("ping -c2 -W3 -I wg0 100.64.0.1");
    assert!(sys.wg_status("wg0").unwrap().peers[0].last_handshake.is_some(), "the tunnel is up");
    let from_b = || b.run("ping -c3 -W1 -i 0.3 100.64.0.2 >/dev/null 2>&1 && echo yes || echo no").trim().to_string();
    assert_eq!(from_b(), "yes", "with no firewall table at all the ping goes through the tunnel (baseline)");
    let mut nf = NfSocket::open(5).expect("open the nf_tables socket");
    let net = Addr::Cidr("100.64.0.0".parse().unwrap(), 24);
    let ctx = Ctx { vpn_nets: vec![net], ..Ctx::default() };
    // the default firewall (ping from the local network only): the tunnel peer is not local, ping is dropped
    let mut cfg = Config::default();
    cfg.presets.clear();
    nf.replace(&build(&cfg, &Resolved::default(), &ctx).unwrap().ruleset).expect("load");
    assert_eq!(from_b(), "no", "without the setting the peer in 100.64.0.0/24 is not the local network");
    // with the setting, the tunnel's network counts, and the same ping gets through
    cfg.vpn_local = true;
    nf.replace(&build(&cfg, &Resolved::default(), &ctx).unwrap().ruleset).expect("load");
    let got = from_b();
    assert_eq!(got, "yes", "with the setting the tunnel's network is the local network; the ruleset:\n{}", build(&cfg, &Resolved::default(), &ctx).unwrap().ruleset.to_text());
    // and the network is only that one: another address of the peer's side is still outside
    let other = Ctx { vpn_nets: vec![Addr::Cidr("100.65.0.0".parse().unwrap(), 24)], ..Ctx::default() };
    nf.replace(&build(&cfg, &Resolved::default(), &other).unwrap().ruleset).expect("load");
    assert_eq!(from_b(), "no", "a tunnel network that does not hold the peer does not open anything for it");
    // setting off again closes it
    cfg.vpn_local = false;
    nf.replace(&build(&cfg, &Resolved::default(), &ctx).unwrap().ruleset).expect("load");
    assert_eq!(from_b(), "no");
    nf.remove().unwrap();
    sys.ip(&["link".into(), "del".into(), "dev".into(), "wg0".into()]).unwrap();
}

#[test]
#[ignore]
fn the_vpn_manager_runs_unchanged_over_the_helper_socket_with_the_real_system_layer_behind_it() {
    use iyakki_agent::gitkeys::{AuditFn, MemStore};
    use iyakki_agent::vpn_mgr::{Manager, State, IFNAME};
    use iyakki_vpn::profile::Settings;
    use std::sync::Arc;
    if !ready() {
        return;
    }
    sh("ip link set lo up");
    let dir = std::path::PathBuf::from(format!("/tmp/iyakki-vpnhelper-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (resolv, backup) = (dir.join("resolv.conf"), dir.join("resolv.backup"));
    std::fs::write(&resolv, "nameserver 192.0.2.53\n").unwrap();
    let b = Ns::new();
    b.run("ip link set lo up");
    sh("ip link add vA type veth peer name vB");
    sh(&format!("ip link set vB netns {}", b.pid));
    sh("ip addr add 10.77.0.1/24 dev vA && ip link set vA up && ip route add default via 10.77.0.2");
    b.run("ip addr add 10.77.0.2/24 dev vB && ip link set vB up && ip addr add 203.0.113.50/32 dev lo");
    let (a_priv, a_pub) = keypair();
    let (b_priv, b_pub) = keypair();
    // the server: wg1 with the tunnel address 10.8.0.1, the client's key allowed as 10.8.0.2/32
    b.run("ip link add wg1 type wireguard");
    {
        let (sk, pk) = (b_priv.clone(), a_pub.clone());
        b.on(move || {
            let sys = OsSys::default();
            let (sk, pk) = (Secret::from_key_b64(&sk).unwrap(), PublicKey::parse(&pk).unwrap());
            let allowed = [Cidr::parse("10.8.0.2/32").unwrap()];
            let mut peer = PeerSet::new(&pk, &allowed);
            peer.replace_allowed_ips = true;
            sys.wg_configure(&DeviceSet { ifname: "wg1", private_key: Some(&sk), listen_port: Some(51820), fwmark: None, replace_peers: true, peers: vec![peer] }).expect("configure the server device");
        });
    }
    b.run("ip addr add 10.8.0.1/24 dev wg1 && ip link set wg1 up");
    let conf = format!("[Interface]\nPrivateKey = {a_priv}\nAddress = 10.8.0.2/24\nDNS = 10.8.0.1\n\n[Peer]\nPublicKey = {b_pub}\nEndpoint = 10.77.0.2:51820\nAllowedIPs = 0.0.0.0/0\nPersistentKeepalive = 5\n");
    // the privileged helper (F-VPN-16): the real OsSys sits behind a Unix socket, the manager only holds the client
    let real: Arc<dyn Sys> = Arc::new(OsSys { resolv_path: resolv.display().to_string(), backup_path: backup.display().to_string() });
    let sock = dir.join("helper").join("vpn.sock");
    let uid = unsafe { libc::getuid() };
    let listener = iyakki_agent::vpn_helper::bind_socket(&sock, Some(uid)).expect("bind the helper socket");
    let helper = iyakki_agent::vpn_helper::serve(listener, sock.clone(), iyakki_agent::vpn_helper::PeerPolicy::only(uid), real).expect("start the helper");
    let sys = Arc::new(iyakki_agent::vpn_helper::RemoteSys::new(&sock));
    let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let l2 = log.clone();
    let audit: AuditFn = Arc::new(move |a, d| l2.lock().unwrap().push(format!("{a} {d}")));
    let vault = Arc::new(MemStore::default());
    let mgr = Manager::new(&dir.join("vpn"), vault, sys.clone(), audit);
    let id = mgr.import("Office", conf.as_bytes()).expect("import").profile.id;
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    mgr.set_settings(&id, Settings { kill_switch: true, ..Settings::full() }, now()).expect("settings");
    assert!(!sh("ip route get 203.0.113.50").contains(IFNAME), "before connecting the address is reached over the cable");
    mgr.set_enabled(&id, true, now()).expect("enable");
    let end = std::time::Instant::now() + std::time::Duration::from_secs(40);
    while std::time::Instant::now() < end && !matches!(mgr.status(&id).state, State::Connected { .. }) {
        mgr.tick(now());
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let st = mgr.status(&id);
    assert!(matches!(st.state, State::Connected { .. }), "the manager did not reach Connected: {st:?}\n{:?}", log.lock().unwrap());
    // the tunnel is the way out: routes, address, DNS, guard
    let route = sh("ip route get 203.0.113.50");
    assert!(route.contains(IFNAME), "everything goes through the tunnel now: {route}");
    assert!(sh("ip -4 -o addr show").contains("10.8.0.2/24") || sh("ip -4 -o addr show").contains("10.8.0.2/"), "tunnel address");
    assert!(sh("ip route get 10.77.0.2").contains("dev vA"), "the server itself is still reached over the cable (pinned endpoint route)");
    let now_resolv = std::fs::read_to_string(&resolv).unwrap();
    assert!(now_resolv.contains("nameserver 10.8.0.1") && !now_resolv.contains("192.0.2.53"), "the VPN's resolver is in place: {now_resolv}");
    assert!(std::fs::read_to_string(&backup).unwrap().contains("192.0.2.53"), "the original resolver list is kept");
    assert!(sys.guard_present() && st.guard, "the kill switch table is in place: {st:?}");
    assert!(ping_ok("10.8.0.1"), "the server's tunnel address answers");
    assert!(ping_ok("203.0.113.50"), "an address behind the server answers through the tunnel");
    let server = b.on(|| OsSys::default().wg_status("wg1").expect("server status"));
    assert!(server.peers[0].last_handshake.is_some() && server.peers[0].rx_bytes > 0, "the server saw the client: {server:?}");
    mgr.tick(now());
    assert!(mgr.status(&id).rx > 0 && mgr.status(&id).tx > 0, "the manager counts the bytes: {:?}", mgr.status(&id));
    // switched off: everything it put on the system is gone, the original state is back
    mgr.set_enabled(&id, false, now()).expect("disable");
    assert!(!sys.link_exists(IFNAME), "the tunnel interface is gone");
    assert!(!sys.guard_present(), "the guard goes with the profile when it is switched off");
    assert_eq!(std::fs::read_to_string(&resolv).unwrap().trim(), "nameserver 192.0.2.53", "the resolver list is back");
    assert!(!sh("ip route get 203.0.113.50").contains(IFNAME), "the default way out is back");
    assert!(matches!(mgr.status(&id).state, State::Stopped), "{:?}", mgr.status(&id));
    // deleting the profile leaves nothing in the vault or on the system
    mgr.remove(&id).expect("remove");
    assert!(!sys.link_exists(IFNAME));
    let ops = helper.log.lock().unwrap().clone();
    assert!(ops.len() > 5, "the helper did the work: {ops:?}");
    drop(helper);
    assert!(!sock.exists(), "the socket is removed when the helper stops");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Packets counted on an interface (`ip -s link`): the line after "TX:" has bytes then packets.
fn tx_packets(ifname: &str) -> u64 {
    let t = sh(&format!("ip -s link show dev {ifname}"));
    let mut lines = t.lines();
    while let Some(l) = lines.next() {
        if l.trim_start().starts_with("TX:") {
            return lines.next().and_then(|n| n.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or(0);
        }
    }
    0
}

#[test]
#[ignore]
fn policy_routing_text_sends_chosen_destinations_and_accounts_through_the_tunnel_interface_in_the_real_kernel() {
    use iyakki_vpn::policyroute::{plan_policy_routes, PolicyInput, DEFAULT_MARK, DEFAULT_PRIORITY, DEFAULT_TABLE};
    if !ready() || !have("nft") {
        return;
    }
    sh("ip link set lo up");
    // the tests of this file share one namespace: whatever this one makes is removed again, also when an assertion fails
    struct Cleanup;
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for c in ["ip -4 rule del fwmark 0x49560002 lookup 18774 priority 10000", "ip -6 rule del fwmark 0x49560002 lookup 18774 priority 10000", "nft delete table inet iyakki_vpn_mark", "ip link del cab0", "ip link del iyvpn0"] {
                let _ = Command::new("sh").args(["-c", &format!("{c} 2>/dev/null")]).status();
            }
        }
    }
    let _cleanup = Cleanup;
    // a cable (dummy `cab0`, the normal default route) and a tunnel interface (dummy `iyvpn0`): packets are counted on each
    sh("ip link add cab0 type dummy && ip addr add 10.55.0.2/24 dev cab0 && ip link set cab0 up && ip route add default via 10.55.0.1 dev cab0 metric 100");
    sh("ip link add iyvpn0 type dummy && ip addr add 10.9.0.2/24 dev iyvpn0 && ip link set iyvpn0 up");
    sh("ip neigh replace 10.55.0.1 lladdr 02:00:00:00:00:01 dev cab0 nud permanent");
    sh("ip neigh replace 10.9.0.1 lladdr 02:00:00:00:00:02 dev iyvpn0 nud permanent");
    let include = [Cidr::parse("198.51.100.0/24").unwrap()];
    let exclude = [Cidr::parse("198.51.100.77/32").unwrap()];
    let plan = plan_policy_routes(&PolicyInput { ifname: "iyvpn0", table: DEFAULT_TABLE, mark: DEFAULT_MARK, priority: DEFAULT_PRIORITY, include: &include, exclude: &exclude, uids: &[4242] }).expect("the plan");
    // the tunnel's own table needs a way out: a default route (the helper's plan adds it too)
    for a in &plan.add {
        let o = std::process::Command::new("ip").args(a).output().unwrap();
        assert!(o.status.success(), "ip {a:?}: {}", String::from_utf8_lossy(&o.stderr));
    }
    let mut nft = std::process::Command::new("nft").args(["-f", "-"]).stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    use std::io::Write as _;
    nft.stdin.take().unwrap().write_all(plan.nft.as_bytes()).unwrap();
    let o = nft.wait_with_output().unwrap();
    assert!(o.status.success(), "nft refused our text: {}", String::from_utf8_lossy(&o.stderr));
    let rules = sh("ip -4 rule show");
    assert!(rules.contains("fwmark 0x49560002") && rules.contains("lookup 18774"), "the rule is in the kernel: {rules}");
    let send = |dst: &str, uid: Option<u32>| {
        let code = format!("import socket,os\nif {u}: os.setuid({u})\ns=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)\ns.settimeout(0.2)\ntry: s.sendto(b'x',('{dst}',9))\nexcept OSError: pass\n", u = uid.unwrap_or(0));
        let _ = std::process::Command::new("python3").args(["-c", &code]).output();
    };
    let (c0, t0) = (tx_packets("cab0"), tx_packets("iyvpn0"));
    send("198.51.100.5", None);
    let (c1, t1) = (tx_packets("cab0"), tx_packets("iyvpn0"));
    assert!(t1 > t0 && c1 == c0, "a chosen destination leaves through the tunnel interface only: cab0 {c0}->{c1}, iyvpn0 {t0}->{t1}");
    send("198.51.100.77", None);
    let (c2, t2) = (tx_packets("cab0"), tx_packets("iyvpn0"));
    assert!(c2 > c1 && t2 == t1, "an excluded address inside the range keeps the cable: cab0 {c1}->{c2}, iyvpn0 {t1}->{t2}");
    send("203.0.113.9", None);
    let (c3, t3) = (tx_packets("cab0"), tx_packets("iyvpn0"));
    assert!(c3 > c2 && t3 == t2, "an address nobody chose keeps the cable: cab0 {c2}->{c3}, iyvpn0 {t2}->{t3}");
    send("203.0.113.9", Some(4242));
    let (c4, t4) = (tx_packets("cab0"), tx_packets("iyvpn0"));
    assert!(t4 > t3 && c4 == c3, "a chosen account goes through the tunnel whatever it sends to: cab0 {c3}->{c4}, iyvpn0 {t3}->{t4}");
    // the plan's model says the same
    use std::net::IpAddr;
    assert!(plan.goes_through_tunnel("198.51.100.5".parse::<IpAddr>().unwrap(), 0, 0) && !plan.goes_through_tunnel("198.51.100.77".parse::<IpAddr>().unwrap(), 0, 0) && plan.goes_through_tunnel("203.0.113.9".parse::<IpAddr>().unwrap(), 4242, 0));
    // undone: no rule, no table, the same destination takes the cable again
    for a in &plan.remove {
        let _ = std::process::Command::new("ip").args(a).output();
    }
    sh("nft delete table inet iyakki_vpn_mark");
    assert!(!sh("ip -4 rule show").contains("fwmark 0x49560002"), "the rule is gone");
    assert!(!sh("nft list tables").contains("iyakki_vpn_mark"), "the nft table is gone");
    send("198.51.100.5", None);
    let (c5, t5) = (tx_packets("cab0"), tx_packets("iyvpn0"));
    assert!(c5 > c4 && t5 == t4, "after the undo the chosen destination takes the cable again: cab0 {c4}->{c5}, iyvpn0 {t4}->{t5}");
}
