//! IYAKKI's mDNS announcer and resolver beside a REAL `avahi-daemon` and a real `dig` (ignored by default).
//!
//! Needs root, `avahi-daemon`, `dig`, `ip`, `nsenter`, `unshare`; run it only through `dist/linux-host/kernel-tests.sh`, which starts it in a throw-away network and mount namespace
//! (the machine's own avahi, network and /run are never touched). The test joins this namespace (A, with our code) to a second one (B, with avahi on the other end of a veth cable;
//! avahi runs without D-Bus, as root, with a tmpfs over its own /run folder). Rows: F-NAME-01 (protocol beside avahi), F-NAME-04 (name lookup), F-DOMAIN-03.
//!   1. our resolver asks for avahi's host name and reads its address;
//!   2. our announcer starts its three probes beside avahi, finishes without a conflict and answers `dig` (a real DNS client) for its host name and its service;
//!   3. an announcer that takes avahi's host name loses it (our probes meet avahi's answer and add a suffix), avahi keeps its name;
//!   4. avahi started later for a name our announcer already holds is the one that renames (our responder defends the name when probed).

use super::net::{resolve, scan, Announcer, Ask};
use super::responder::Svc;
use std::net::IpAddr;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn sh(cmd: &str) -> (bool, String) {
    let o = Command::new("sh").args(["-c", &format!("export PATH=$PATH:/usr/sbin:/sbin; {cmd}")]).output().unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

fn wait<F: FnMut() -> bool>(secs: u64, mut f: F) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

struct Avahi {
    child: Child,
    log: std::path::PathBuf,
}

impl Avahi {
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Avahi {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_avahi(pid: u32, dir: &std::path::Path, host: &str) -> Avahi {
    let conf = dir.join(format!("avahi-{host}.conf"));
    std::fs::write(&conf, format!("[server]\nhost-name={host}\nuse-ipv4=yes\nuse-ipv6=no\nallow-interfaces=vB\nenable-dbus=no\ncheck-response-ttl=no\n[publish]\npublish-hinfo=no\npublish-workstation=no\n[wide-area]\nenable-wide-area=no\n")).unwrap();
    let log = dir.join(format!("avahi-{host}.log"));
    let child = Command::new("nsenter")
        .args(["-t", &pid.to_string(), "-n", "/usr/sbin/avahi-daemon", "-f"])
        .arg(&conf)
        .args(["--no-drop-root", "--no-chroot"])
        .stdout(Stdio::from(std::fs::File::create(&log).unwrap()))
        .stderr(Stdio::from(std::fs::File::options().append(true).open(&log).unwrap()))
        .spawn()
        .expect("avahi-daemon");
    Avahi { child, log }
}

#[test]
#[ignore]
fn our_announcer_and_resolver_work_beside_a_real_avahi_daemon_and_answer_a_real_dig() {
    // SAFETY: getuid has no preconditions.
    let root = unsafe { libc::getuid() } == 0 && std::env::var("IYAKKI_IN_SANDBOX").as_deref() == Ok("1");
    if !(root && sh("command -v avahi-daemon >/dev/null && command -v dig >/dev/null && command -v nsenter >/dev/null && command -v unshare >/dev/null && command -v ip >/dev/null").0) {
        eprintln!("skipped: needs root, avahi-daemon, dig, ip, nsenter and unshare (run it through dist/linux-host/kernel-tests.sh)");
        return;
    }
    let dir = std::path::PathBuf::from(format!("/tmp/iyakki-avahi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    sh("ip link set lo up");
    let mut other = Command::new("unshare").args(["-n", "sleep", "300"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let pid = other.id();
    let (ok, out) = sh(&format!(
        "ip link add vA type veth peer name vB && ip link set vB netns {pid} && ip addr add 10.77.0.1/24 dev vA && ip link set vA up && ip link set vA multicast on && \
         nsenter -t {pid} -n sh -c 'ip link set lo up && ip addr add 10.77.0.2/24 dev vB && ip link set vB up && ip link set vB multicast on' && \
         mkdir -p /run/avahi-daemon && mount -t tmpfs tmpfs /run/avahi-daemon"
    ));
    assert!(ok, "{out}");
    let dig = |name: &str, kind: &str| sh(&format!("nsenter -t {pid} -n dig +short +time=3 +tries=1 -p 5353 @10.77.0.1 {name} {kind}")).1;

    // 1. our resolver reads avahi's own host name
    let avahi = start_avahi(pid, &dir, "avahitest");
    assert!(wait(15, || avahi.log().contains("Server startup complete")), "avahi did not start:\n{}", avahi.log());
    let ask = Ask::new(Duration::from_secs(3));
    let got = resolve("avahitest.local", &ask);
    assert!(got.contains(&"10.77.0.2".parse::<IpAddr>().unwrap()), "our resolver got {got:?} from avahi; avahi log:\n{}", avahi.log());

    // 2. our announcer beside avahi: no conflict, and a real DNS client reads its answers
    let svc = Svc { kind: "_iyakki-dev._tcp".into(), port: 7979, txt: vec!["v=1".into()] };
    let mut ours = Announcer::start("iyakki-t1e2s3", vec![svc]).expect("announcer on 5353 beside avahi");
    assert!(wait(10, || !ours.status().probing), "probing did not finish: {:?}", ours.status());
    let st = ours.status();
    assert_eq!(st.host, "iyakki-t1e2s3.local", "no conflict was expected: {st:?}");
    assert!(st.v4.iter().any(|a| a.to_string() == "10.77.0.1"), "{st:?}");
    let a = dig("iyakki-t1e2s3.local", "A");
    assert!(a.contains("10.77.0.1"), "dig asked our responder for the host name and got: {a:?}");
    let ptr = dig("_iyakki-dev._tcp.local", "PTR");
    assert!(ptr.contains("iyakki-t1e2s3._iyakki-dev._tcp.local"), "dig asked for the service and got: {ptr:?}");
    let srv = dig("iyakki-t1e2s3._iyakki-dev._tcp.local", "SRV");
    assert!(srv.contains("7979") && srv.contains("iyakki-t1e2s3.local"), "SRV answer: {srv:?}");
    // `iyakki dev scan` from the other computer: a thread that joined B's network namespace asks the cable with the real scan and finds our device, its address and its service
    let found = std::thread::spawn(move || {
        let path = std::ffi::CString::new(format!("/proc/{pid}/ns/net")).unwrap();
        // SAFETY: open and setns on a path made above; the descriptor is closed after use.
        unsafe {
            let fd = libc::open(path.as_ptr(), libc::O_RDONLY);
            assert!(fd >= 0, "open the namespace");
            assert_eq!(libc::setns(fd, libc::CLONE_NEWNET), 0, "setns");
            libc::close(fd);
        }
        scan(&Ask::new(Duration::from_secs(3)))
    })
    .join()
    .expect("the scan thread");
    let me = found.iter().find(|f| f.host == "iyakki-t1e2s3.local").unwrap_or_else(|| panic!("the scan from the other computer did not find our device: {found:?}"));
    assert!(me.addrs.iter().any(|a| a.to_string() == "10.77.0.1"), "{me:?}");
    assert!(me.services.iter().any(|(k, port)| k.contains("_iyakki-dev._tcp") && *port == 7979), "{me:?}");
    ours.stop();
    drop(ours);

    // 3. an announcer that takes avahi's host name loses it; avahi keeps it
    let mut rival = Announcer::start("avahitest", vec![]).unwrap();
    let renamed = wait(15, || {
        let s = rival.status();
        !s.probing && s.host != "avahitest.local"
    });
    let rs = rival.status();
    assert!(renamed && rs.host.starts_with("avahitest-"), "our announcer should have added a suffix after avahi's answer: {rs:?}");
    assert!(!avahi.log().contains("Host name conflict"), "avahi should have kept its name:\n{}", avahi.log());
    let still: Vec<IpAddr> = resolve("avahitest.local", &ask).into_iter().filter(|a| a.is_ipv4()).collect();
    assert_eq!(still, vec!["10.77.0.2".parse::<IpAddr>().unwrap()], "avahitest.local must still be avahi's only: {still:?}");
    rival.stop();
    drop(rival);
    drop(avahi);

    // 4. avahi started for a name that our announcer holds is the one to rename
    let mut held = Announcer::start("iyakki-held", vec![]).unwrap();
    assert!(wait(10, || !held.status().probing), "{:?}", held.status());
    assert_eq!(held.status().host, "iyakki-held.local");
    let late = start_avahi(pid, &dir, "iyakki-held");
    let conflict = wait(20, || late.log().contains("conflict") || late.log().contains("iyakki-held-2"));
    assert!(conflict, "avahi never met our claim on the name; its log:\n{}", late.log());
    assert_eq!(held.status().host, "iyakki-held.local", "our announcer keeps its name when a stranger claims it: {:?}", held.status());
    held.stop();
    drop(held);
    drop(late);
    let _ = other.kill();
    let _ = other.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
