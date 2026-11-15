//! Internet sharing (`sharing::manager` with the real `RealOs`) against a REAL kernel, a real DHCP client, a real DNS resolver tool and real NAT (ignored by default).
//!
//! Needs root, `ip`, `nsenter`, `unshare`, `python3`, `dhcpcd`, `dnsmasq` (as a plain DNS server for the uplink side), `dig` and `nft`; run it only through
//! `dist/linux-host/kernel-tests.sh`, which starts it in a throw-away network and mount namespace (the machine's own network, firewall, /run and /etc/resolv.conf are never
//! touched: this test mounts tmpfs and bind mounts only inside that namespace). Three computers: A (this namespace, the one that shares), U (the upstream, 198.51.100.2, a web
//! listener and a DNS server; A's default route goes there) and C (the device on the cable, which only has `dhcpcd`). The real manager serves DHCP and DNS on sockets bound to the
//! cable, loads its table `inet iyakki_share` and switches on forwarding; the test then checks what really happens to C's packets. Rows: F-HOTSPOT-10, -11, -13, -06.

#![cfg(target_os = "linux")]

use iyakki_agent::sharing::manager::Manager;
use iyakki_agent::sharing::real::RealOs;
use iyakki_agent::sharing::{Request, State, Status};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn sh(cmd: &str) -> (bool, String) {
    let o = Command::new("sh").args(["-c", &format!("export PATH=$PATH:/usr/sbin:/sbin; {cmd}")]).output().unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

fn must(cmd: &str) -> String {
    let (ok, out) = sh(cmd);
    assert!(ok, "{cmd}: {out}");
    out
}

/// Kills what the test started, also when an assertion fails halfway (a panic would otherwise leave them running).
struct Pids(Vec<u32>);

impl Drop for Pids {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = Command::new("kill").args(["-9", &p.to_string()]).stderr(Stdio::null()).status();
        }
    }
}

fn spawn_ns() -> Child {
    let c = Command::new("unshare").args(["-n", "sleep", "600"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    c
}

/// Does a TCP connection from namespace `pid` to `ip:port` work within two seconds?
fn tcp_ok(pid: u32, ip: &str, port: u16) -> bool {
    sh(&format!("nsenter -t {pid} -n python3 -c \"import socket;s=socket.socket();s.settimeout(2);s.connect(('{ip}',{port}));s.close()\"")).0
}

const SERVE: &str = "import socket,threading,sys,time\nlog=sys.argv[1]\ndef tcp(p):\n    s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('0.0.0.0',p)); s.listen(20)\n    while True:\n        c,a=s.accept(); open(log,'a').write('%d %s\\n'%(p,a[0])); c.close()\nfor p in (8080,7777): threading.Thread(target=tcp,args=(p,),daemon=True).start()\ntime.sleep(900)\n";

fn wait<F: FnMut() -> bool>(secs: u64, mut f: F) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

#[test]
#[ignore]
fn the_sharing_manager_gives_a_device_an_address_a_resolver_and_nat_and_keeps_it_off_this_computer_and_the_lan() {
    // SAFETY: getuid has no preconditions.
    let root = unsafe { libc::getuid() } == 0;
    let tools = ["python3", "nsenter", "unshare", "ip", "dhcpcd", "dnsmasq", "dig", "nft"].iter().all(|t| sh(&format!("command -v {t} >/dev/null")).0);
    if !(root && tools && std::env::var("IYAKKI_IN_SANDBOX").as_deref() == Ok("1")) {
        eprintln!("skipped: needs root, ip, nsenter, unshare, python3, dhcpcd, dnsmasq, dig, nft and the sandbox of dist/linux-host/kernel-tests.sh");
        return;
    }
    let dir = std::path::PathBuf::from(format!("/tmp/iyakki-share-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("empty.conf"), "").unwrap();
    // private places for what dhcpcd and the resolver list would otherwise touch (this mount namespace only)
    let resolved = must("realpath -m /etc/resolv.conf").trim().to_string();
    must("mount -t tmpfs tmpfs /run");
    std::fs::write(dir.join("resolv.conf"), "nameserver 198.51.100.2\n").unwrap();
    if resolved.starts_with("/run/") {
        // /etc/resolv.conf points into /run (systemd-resolved): the private tmpfs now holds our file at that place
        must(&format!("mkdir -p $(dirname {resolved}) && cp {} {resolved}", dir.join("resolv.conf").display()));
    } else {
        must(&format!("mount --bind {} {resolved}", dir.join("resolv.conf").display()));
    }
    for d in ["/var/lib/dhcpcd", "/var/db/dhcpcd"] {
        if std::path::Path::new(d).is_dir() {
            must(&format!("mount -t tmpfs tmpfs {d}"));
        }
    }
    sh("ip link set lo up");
    let (mut up, mut dev) = (spawn_ns(), spawn_ns());
    let (upid, cid) = (up.id(), dev.id());
    // uplink: A's eth0 <-> U's u0 (198.51.100.0/24, a public range); A's default route goes to U. U also owns a private address (10.50.0.2) behind it.
    must(&format!(
        "ip link add eth0 type veth peer name u0 && ip link set u0 netns {upid} && ip addr add 198.51.100.1/24 dev eth0 && ip link set eth0 up && ip route add default via 198.51.100.2 && ip route add 10.50.0.2/32 via 198.51.100.2 && \
         nsenter -t {upid} -n sh -c 'ip link set lo up && ip addr add 198.51.100.2/24 dev u0 && ip link set u0 up && ip addr add 10.50.0.2/32 dev lo'"
    ));
    // the cable: A's eth7 (left down, no address: the manager takes it) <-> the device's c0
    must(&format!("ip link add eth7 type veth peer name c0 && ip link set c0 netns {cid} && nsenter -t {cid} -n sh -c 'ip link set lo up && ip link set c0 up'"));
    let log = dir.join("u-heard.txt");
    let mut web = Command::new("nsenter").args(["-t", &upid.to_string(), "-n", "python3", "-c", SERVE, log.to_str().unwrap()]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let mut dns = Command::new("nsenter")
        .args(["-t", &upid.to_string(), "-n", "dnsmasq", "-k", "--no-resolv", "--no-hosts", &format!("--conf-file={}", dir.join("empty.conf").display()), "--address=/probe.test/198.51.100.2", "--listen-address=198.51.100.2", "--bind-interfaces", "--port=53", &format!("--pid-file={}", dir.join("dnsmasq.pid").display())])
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    // a listener on A itself (all addresses): the device must not reach it
    let mut local = Command::new("python3").args(["-c", SERVE, dir.join("a-heard.txt").to_str().unwrap()]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(600));
    let _kill_on_exit = Pids(vec![up.id(), dev.id(), web.id(), dns.id(), local.id()]);
    let forward_before = std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward").unwrap().trim().to_string();

    // the real manager, ticking a few times a second
    let req = Arc::new(Mutex::new(Request { id: 1, on: true, uplink: Some("eth0".into()), via: Some("eth7".into()), isolate: true, allow_lan: false, by: "admin".into(), by_admin: true, ..Request::default() }));
    let status = Arc::new(Mutex::new(Status::default()));
    let stop = Arc::new(AtomicBool::new(false));
    let (r2, s2, st2, data) = (req.clone(), status.clone(), stop.clone(), dir.join("data"));
    let mgr = std::thread::spawn(move || {
        let mut os = RealOs::new(&data);
        let mut m = Manager::new();
        while !st2.load(Ordering::SeqCst) {
            let r = r2.lock().unwrap().clone();
            *s2.lock().unwrap() = m.tick(&mut os, &r);
            std::thread::sleep(Duration::from_millis(100));
        }
        m.shutdown(&mut os);
    });
    let ok = wait(30, || status.lock().unwrap().state == State::On);
    let st = status.lock().unwrap().clone();
    assert!(ok, "the session did not start: {st:?}");
    assert_eq!((st.uplink.as_str(), st.downstream.as_str()), ("eth0", "eth7"));
    let gw = st.gateway.clone();
    assert!(!gw.is_empty(), "{st:?}");

    // the device asks for an address with a real DHCP client
    let empty = dir.join("empty.conf");
    let (ok, out) = sh(&format!("nsenter -t {cid} -n dhcpcd -1 -4 -c /bin/true -f {} -t 20 c0 2>&1", empty.display()));
    let addrs = sh(&format!("nsenter -t {cid} -n ip -4 -o addr show dev c0")).1;
    assert!(addrs.contains(&gw[..gw.rfind('.').unwrap()]), "dhcpcd did not get an address in {gw}/24 (ok {ok}):\n{out}\n{addrs}");
    let routes = sh(&format!("nsenter -t {cid} -n ip route show")).1;
    assert!(routes.contains(&format!("default via {gw}")), "the lease should give the cable's computer as the router:\n{routes}");
    assert!(wait(10, || status.lock().unwrap().clients.iter().any(|c| !c.ip.is_empty())), "the status should list the device: {:?}", status.lock().unwrap());

    // DNS: the device asks this computer, which asks the upstream resolver (dnsmasq in U)
    let a = sh(&format!("nsenter -t {cid} -n dig +short +time=3 +tries=1 @{gw} probe.test A")).1;
    assert!(a.trim() == "198.51.100.2", "the forwarder should relay the upstream answer: {a:?}");
    // NAT: the device reaches the upstream, which sees this computer's uplink address, not the device's
    assert!(tcp_ok(cid, "198.51.100.2", 8080), "the device should reach the public upstream through this computer");
    let heard = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(heard.lines().any(|l| l == "8080 198.51.100.1"), "the upstream must see the masqueraded address 198.51.100.1: {heard:?}");
    // what it must NOT reach: a service on this computer, and the uplink's private range
    assert!(!tcp_ok(cid, &gw, 7777), "the device must not reach a listener on this computer");
    assert!(!tcp_ok(cid, "10.50.0.2", 8080), "the uplink's private range is dropped unless allowed");
    let table = must("nft list table inet iyakki_share");
    assert!(table.contains("masquerade"), "{table}");

    // "allow the local network": a new request restarts the session; the private address is reachable now
    {
        let mut r = req.lock().unwrap();
        r.id = 2;
        r.allow_lan = true;
    }
    assert!(wait(30, || { let s = status.lock().unwrap(); s.state == State::On && s.request_id == 2 }), "restart: {:?}", status.lock().unwrap());
    assert!(wait(10, || tcp_ok(cid, "10.50.0.2", 8080)), "with the local network allowed the private range is reachable");
    assert!(tcp_ok(cid, "198.51.100.2", 8080));
    assert!(!tcp_ok(cid, &gw, 7777), "still no access to this computer");

    // off: everything goes back
    req.lock().unwrap().on = false;
    req.lock().unwrap().id = 3;
    assert!(wait(20, || status.lock().unwrap().state == State::Off), "{:?}", status.lock().unwrap());
    assert!(!sh("nft list table inet iyakki_share").0, "the table must be gone");
    assert_eq!(std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward").unwrap().trim(), forward_before, "ip_forward goes back to what it was");
    assert!(!sh("ip -4 -o addr show dev eth7").1.contains(&gw), "the cable's address is removed");
    stop.store(true, Ordering::SeqCst);
    mgr.join().unwrap();
    for c in [&mut web, &mut dns, &mut local, &mut up, &mut dev] {
        let _ = c.kill();
        let _ = c.wait();
    }
    let _ = std::fs::remove_dir_all(&dir);
}
