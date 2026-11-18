//! The firewall's own packet model against a REAL kernel with real cables (ignored by default).
//!
//! Needs root, `python3`, `ip`, `nsenter`, `unshare` and nf_tables; run it only through `dist/linux-host/kernel-tests.sh`, which starts it in a throw-away network namespace
//! (the machine's own firewall and network are never touched). The test joins this namespace (A) to two others by veth pairs, B on a "direct link" interface (`vA`, one cable
//! to one other computer) and C on a second interface that is not a direct link, loads rulesets through the in-house netlink writer, and from B and C tries a grid of TCP
//! connections, UDP datagrams and pings. What really arrives must equal what `ruleset::eval` says for the same packet. Rows: F-FW-12 (direct-link zone), F-FW-02, F-FW-03.

#![cfg(target_os = "linux")]

use iyakki_fw::model::{Config, Kind, Proto, Resolved, Rule};
use iyakki_fw::nl::NfSocket;
use iyakki_fw::ruleset::{build, eval, Ctx, Packet, CT_NEW};
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

const TCP_PORTS: [u16; 7] = [22, 80, 7979, 7980, 8000, 8050, 8443];
const UDP_PORTS: [u16; 4] = [53, 5353, 7979, 7980];

/// Listeners in this namespace: every TCP port of the grid accepts, every UDP port of the grid echoes.
const LISTEN: &str = r#"
import socket, threading, time
def tcp(p):
    s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(('0.0.0.0', p)); s.listen(20)
    while True:
        c, _ = s.accept(); c.close()
def udp(p):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.bind(('0.0.0.0', p))
    while True:
        d, a = s.recvfrom(100); s.sendto(d, a)
for p in %TCP%: threading.Thread(target=tcp, args=(p,), daemon=True).start()
for p in %UDP%: threading.Thread(target=udp, args=(p,), daemon=True).start()
time.sleep(600)
"#;

fn probe(pid: u32, dst: &str) -> Vec<(String, u16, bool)> {
    // one python process in the other namespace tries the whole grid; true = the connection or datagram got through
    let py = format!(
        "import socket,sys\nfor p in {tcp:?}:\n    s=socket.socket(); s.settimeout(1.0)\n    try:\n        s.connect(('{dst}',p)); print('tcp',p,1)\n    except Exception:\n        print('tcp',p,0)\n    s.close()\nfor p in {udp:?}:\n    s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.settimeout(1.0)\n    try:\n        s.sendto(b'x',('{dst}',p)); s.recvfrom(10); print('udp',p,1)\n    except Exception:\n        print('udp',p,0)\n    s.close()\n",
        tcp = TCP_PORTS,
        udp = UDP_PORTS,
        dst = dst
    );
    let o = Command::new("nsenter").args(["-t", &pid.to_string(), "-n", "python3", "-c", &py]).output().unwrap();
    let mut v: Vec<(String, u16, bool)> = String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        (w.len() == 3).then(|| (w[0].to_string(), w[1].parse().unwrap(), w[2] == "1"))
    }).collect();
    let ping = Command::new("nsenter").args(["-t", &pid.to_string(), "-n", "ping", "-c1", "-W1", dst]).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success();
    v.push(("icmp".into(), 0, ping));
    v
}

fn expected(rs: &iyakki_fw::ruleset::Ruleset, iif: &str, src: &str, what: &(String, u16, bool)) -> bool {
    let (l4, port) = match what.0.as_str() {
        "tcp" => (6, what.1),
        "udp" => (17, what.1),
        _ => (1, 0),
    };
    eval(rs, &Packet { iif: iif.into(), ct: CT_NEW, src: src.parse().unwrap(), l4, dport: port, icmp_type: 8 })
}

#[test]
#[ignore]
fn a_direct_link_zone_and_the_page_rules_decide_real_packets_exactly_like_the_packet_model() {
    // SAFETY: getuid has no preconditions.
    let root = unsafe { libc::getuid() } == 0 && std::env::var("IYAKKI_IN_SANDBOX").as_deref() == Ok("1");
    if !(root && sh("command -v python3 >/dev/null && command -v nsenter >/dev/null && command -v unshare >/dev/null && command -v ip >/dev/null && command -v ping >/dev/null").0) {
        eprintln!("skipped: needs root, python3, ip, ping, nsenter and unshare (run it through dist/linux-host/kernel-tests.sh)");
        return;
    }
    sh("ip link set lo up");
    let (mut b, mut c) = (spawn_ns(), spawn_ns());
    let (bp, cp) = (b.id(), c.id());
    for (pid, a_if, n_if, a_ip, n_ip) in [(bp, "vA", "vB", "169.254.7.1/24", "169.254.7.2/24"), (cp, "vA2", "vC", "10.88.0.1/24", "10.88.0.2/24")] {
        let (ok, out) = sh(&format!("ip link add {a_if} type veth peer name {n_if} && ip link set {n_if} netns {pid} && ip addr add {a_ip} dev {a_if} && ip link set {a_if} up && nsenter -t {pid} -n sh -c 'ip link set lo up && ip addr add {n_ip} dev {n_if} && ip link set {n_if} up'"));
        assert!(ok, "{out}");
    }
    let script = LISTEN.replace("%TCP%", &format!("{TCP_PORTS:?}")).replace("%UDP%", &format!("{UDP_PORTS:?}"));
    let mut listener = Command::new("python3").args(["-c", &script]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));

    // the page's rules: one network may reach the 8000 range; the direct link comes from the Ctx
    let mut cfg = Config::default();
    cfg.presets.clear();
    let mut r = Rule::new(Kind::Cidr, "10.88.0.0/24");
    r.proto = Proto::Tcp;
    r.ports = "8000-8100".into();
    cfg.add(r);
    let mut s = NfSocket::open(5).expect("open the nf_tables socket");
    let mut checked = 0;
    for allow_more in [false, true] {
        let ctx = Ctx { direct_ifaces: vec!["vA".into()], direct_allow_more: allow_more, ..Ctx::default() };
        let rs = build(&cfg, &Resolved::default(), &ctx).unwrap().ruleset;
        s.replace(&rs).unwrap_or_else(|e| panic!("load into the real kernel (allow_more {allow_more}): {e}"));
        // from B (the cable): the zone's rules; from C (another interface, a private address with a page rule for 8000-8100)
        for (pid, dst, iif, src) in [(bp, "169.254.7.1", "vA", "169.254.7.2"), (cp, "10.88.0.1", "vA2", "10.88.0.2")] {
            for what in probe(pid, dst) {
                let want = expected(&rs, iif, src, &what);
                assert_eq!(what.2, want, "allow_more {allow_more}: {} {} from {src} on {iif}: the kernel let it through = {}, the packet model said {want}", what.0, what.1, what.2);
                checked += 1;
            }
        }
        s.remove().unwrap();
    }
    // and the zone really is narrow by default: developer mode, the beacon and ping on the cable, nothing else
    let ctx = Ctx { direct_ifaces: vec!["vA".into()], ..Ctx::default() };
    let rs = build(&cfg, &Resolved::default(), &ctx).unwrap().ruleset;
    s.replace(&rs).unwrap();
    let got = probe(bp, "169.254.7.1");
    let open: Vec<String> = got.iter().filter(|g| g.2).map(|g| format!("{}/{}", g.0, g.1)).collect();
    assert_eq!(open, vec!["tcp/7979", "udp/7980", "icmp/0"], "on a direct link only developer mode, the beacon and ping get in");
    s.remove().unwrap();
    eprintln!("{checked} packets compared with the packet model");
    let _ = listener.kill();
    let _ = listener.wait();
    let _ = b.kill();
    let _ = c.kill();
    let _ = (b.wait(), c.wait());
}
