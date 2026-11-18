//! The domain listener's firewall gate (`gaterule`) against a REAL kernel with real cables (ignored by default).
//!
//! Needs root, `python3`, `ip`, `nsenter` and `unshare`; run it only through `dist/linux-host/kernel-tests.sh`, which starts it in a throw-away network namespace (the machine's own
//! firewall and network are never touched). A TCP listener on 0.0.0.0:8080 and 8443 in this namespace (A) is reached from a computer on a private network (B) and from one on a
//! public address (C), through veth cables. The gate rules are made by `gaterule::plan` for the local network, then for everybody, then removed, and loaded with the in-house netlink
//! writer: before any gate rule both are kept out; with the local network only B gets in; with `Anywhere` both; a port with no route stays shut; gone again after the gate is
//! removed. A rule with an end time that has passed is not in the ruleset. Rows: F-DOMAIN-02, F-DOMAIN-03.

#![cfg(target_os = "linux")]

use iyakki_fw::gaterule::{plan, plan_until, Reach};
use iyakki_fw::model::{Config, Resolved};
use iyakki_fw::nl::NfSocket;
use iyakki_fw::ruleset::{build, Ctx};
use std::collections::BTreeSet;
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

const LISTEN: &str = "import socket,threading,time\ndef tcp(p):\n    s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('0.0.0.0',p)); s.listen(20)\n    while True:\n        c,_=s.accept(); c.close()\nfor p in (8080,8443,9000): threading.Thread(target=tcp,args=(p,),daemon=True).start()\ntime.sleep(600)\n";

/// which of the ports 8080, 8443 and 9000 the other namespace can connect to
fn reach(pid: u32, dst: &str) -> Vec<u16> {
    let py = format!("import socket\nfor p in (8080,8443,9000):\n    s=socket.socket(); s.settimeout(1.0)\n    try:\n        s.connect(('{dst}',p)); print(p)\n    except Exception:\n        pass\n    s.close()\n");
    let o = Command::new("nsenter").args(["-t", &pid.to_string(), "-n", "python3", "-c", &py]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect()
}

fn ports(v: &[u16]) -> BTreeSet<u16> {
    v.iter().copied().collect()
}

#[test]
#[ignore]
fn the_domain_listeners_gate_opens_its_ports_to_the_local_network_or_to_everybody_and_closes_again_on_a_real_kernel() {
    // SAFETY: getuid has no preconditions.
    let root = unsafe { libc::getuid() } == 0 && std::env::var("IYAKKI_IN_SANDBOX").as_deref() == Ok("1");
    if !(root && sh("command -v python3 >/dev/null && command -v nsenter >/dev/null && command -v unshare >/dev/null && command -v ip >/dev/null").0) {
        eprintln!("skipped: needs root, python3, ip, nsenter and unshare (run it through dist/linux-host/kernel-tests.sh)");
        return;
    }
    sh("ip link set lo up");
    let (mut b, mut c) = (spawn_ns(), spawn_ns());
    let (bp, cp) = (b.id(), c.id());
    for (pid, a_if, n_if, a_ip, n_ip) in [(bp, "gA", "gB", "10.88.0.1/24", "10.88.0.2/24"), (cp, "gA2", "gC", "198.51.100.1/24", "198.51.100.2/24")] {
        let (ok, out) = sh(&format!("ip link add {a_if} type veth peer name {n_if} && ip link set {n_if} netns {pid} && ip addr add {a_ip} dev {a_if} && ip link set {a_if} up && nsenter -t {pid} -n sh -c 'ip link set lo up && ip addr add {n_ip} dev {n_if} && ip link set {n_if} up'"));
        assert!(ok, "{out}");
    }
    let mut listener = Command::new("python3").args(["-c", LISTEN]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));

    let mut base = Config::default();
    base.presets.clear();
    let ctx = Ctx { now: 1_000_000, ..Ctx::default() };
    let mut s = NfSocket::open(5).expect("open the nf_tables socket");
    let mut load = |cfg: &Config| {
        let rs = build(cfg, &Resolved::default(), &ctx).unwrap().ruleset;
        s.replace(&rs).unwrap_or_else(|e| panic!("load into the real kernel: {e}"));
    };
    let both = |label: &str| (reach(bp, "10.88.0.1"), reach(cp, "198.51.100.1"), label.to_string());

    // no gate rule: the firewall keeps both out
    load(&base);
    let (private, public, _) = both("no gate");
    assert_eq!((private, public), (Vec::<u16>::new(), Vec::<u16>::new()), "before any gate rule nothing may get in");
    // the gate for two ports, local network only
    let (cfg, changes) = plan(&base, &ports(&[8080, 8443]), Reach::LocalNetwork, ctx.ipv6);
    assert!(!changes.is_empty());
    load(&cfg);
    let (private, public, _) = both("local");
    assert_eq!(private, vec![8080, 8443], "the private network gets the two gated ports and not 9000");
    assert_eq!(public, Vec::<u16>::new(), "a public address stays out while the gate is for the local network");
    // widened to everybody (a listener with public routes)
    let (cfg_all, _) = plan(&cfg, &ports(&[8080, 8443]), Reach::Anywhere, ctx.ipv6);
    load(&cfg_all);
    let (private, public, _) = both("anywhere");
    assert_eq!((private, public.clone()), (vec![8080, 8443], vec![8080, 8443]), "everybody gets the gated ports, never 9000");
    // a port that lost its last route closes, the other one stays
    let (cfg_one, changes) = plan(&cfg_all, &ports(&[8443]), Reach::Anywhere, ctx.ipv6);
    assert!(changes.iter().any(|c| c.0 == "cloudlisten.firewall.removed"));
    load(&cfg_one);
    let (private, public, _) = both("one port");
    assert_eq!((private, public), (vec![8443], vec![8443]));
    // an opening whose end time has passed is not in the kernel
    let (cfg_old, _) = plan_until(&base, &ports(&[8080]), Reach::LocalNetwork, ctx.ipv6, 1);
    let rs = build(&cfg_old, &Resolved::default(), &ctx).unwrap().ruleset.to_text();
    assert!(!rs.contains("8080"), "a rule with an end time in the past must not be loaded:\n{rs}");
    load(&cfg_old);
    let (private, _, _) = both("expired");
    assert_eq!(private, Vec::<u16>::new(), "the expired opening lets nobody in");
    // the gate removed: shut again
    let (cfg_none, _) = plan(&cfg_all, &ports(&[]), Reach::LocalNetwork, ctx.ipv6);
    load(&cfg_none);
    let (private, public, _) = both("closed");
    assert_eq!((private, public), (Vec::<u16>::new(), Vec::<u16>::new()));
    s.remove().unwrap();
    let _ = listener.kill();
    let _ = listener.wait();
    let _ = (b.kill(), c.kill());
    let _ = (b.wait(), c.wait());
}
