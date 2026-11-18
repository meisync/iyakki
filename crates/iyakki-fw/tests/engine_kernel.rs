//! The firewall engine (`Engine`: apply, the 60-second Keep/Revert, emergency off, saved settings, audit lines) against a REAL kernel (ignored by default).
//!
//! Needs root, `python3`, `ip`, `nsenter`, `unshare`; run it only through `dist/linux-host/kernel-tests.sh`, which starts it in a throw-away network namespace (the machine's own
//! firewall is never touched). The engine runs here with a fake clock, a fake resolver and a memory store, but a REAL backend: rulesets go into the kernel through the in-house
//! netlink writer. A listener on the remote-desktop port is reached from a second namespace (a private address, so the preset lets it in). The case of the row: a change that cuts the
//! remote desktop off is applied, really cuts it, is not kept, and the engine puts the old rules back by itself after 60 seconds; a change that is kept stays and is saved; the
//! emergency switch takes the table out. Row: F-FW-04.

#![cfg(target_os = "linux")]

use iyakki_fw::backend::{Backend, BackendError};
use iyakki_fw::engine::{Clock, Engine, Resolver, Saved, Status, Store};
use iyakki_fw::model::Config;
use iyakki_fw::nl::NfSocket;
use iyakki_fw::ruleset::{Ctx, Ruleset};
use std::net::IpAddr;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

fn sh(cmd: &str) -> (bool, String) {
    let o = Command::new("sh").args(["-c", &format!("export PATH=$PATH:/usr/sbin:/sbin; {cmd}")]).output().unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

/// The in-house netlink writer as the engine's backend (the same calls as `KernelBackend`, without its `modprobe` of the host's modules).
struct Kb;

fn conv(e: iyakki_fw::nl::NlError) -> BackendError {
    match e {
        iyakki_fw::nl::NlError::Unavailable(s) => BackendError::Unavailable(s),
        iyakki_fw::nl::NlError::Failed(s) => BackendError::Failed(s),
    }
}

impl Backend for Kb {
    fn replace(&mut self, rs: &Ruleset) -> Result<(), BackendError> {
        NfSocket::open(5).map_err(conv)?.replace(rs).map_err(conv)
    }
    fn remove(&mut self) -> Result<(), BackendError> {
        NfSocket::open(5).map_err(conv)?.remove().map_err(conv)
    }
    fn counters(&mut self) -> Result<(u64, u64), BackendError> {
        NfSocket::open(5).map_err(conv)?.counters().map_err(conv)
    }
}

struct Clk(Arc<AtomicI64>);
impl Clock for Clk {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct NoDns;
impl Resolver for NoDns {
    fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, String> {
        Err("no resolver in this test".into())
    }
}

struct Mem(Arc<Mutex<Option<Saved>>>);
impl Store for Mem {
    fn load(&self) -> Result<Option<Saved>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, s: &Saved) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(s.clone());
        Ok(())
    }
}

const LISTEN: &str = "import socket,time\ns=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('0.0.0.0',8443)); s.listen(20)\nwhile True:\n    c,_=s.accept(); c.close()\n";

fn reach(pid: u32) -> bool {
    sh(&format!("nsenter -t {pid} -n python3 -c \"import socket;s=socket.socket();s.settimeout(1.5);s.connect(('10.88.0.1',8443));s.close()\"")).0
}

fn ping(pid: u32) -> bool {
    sh(&format!("nsenter -t {pid} -n ping -c1 -W1 10.88.0.1")).0
}

#[test]
#[ignore]
fn the_engine_applies_to_a_real_kernel_reverts_a_cut_off_remote_desktop_by_itself_keeps_what_is_kept_and_the_emergency_switch_removes_the_table() {
    // SAFETY: getuid has no preconditions.
    let root = unsafe { libc::getuid() } == 0 && std::env::var("IYAKKI_IN_SANDBOX").as_deref() == Ok("1");
    if !(root && sh("command -v python3 >/dev/null && command -v nsenter >/dev/null && command -v unshare >/dev/null && command -v ip >/dev/null && command -v nft >/dev/null").0) {
        eprintln!("skipped: needs root, python3, ip, nft, nsenter and unshare (run it through dist/linux-host/kernel-tests.sh)");
        return;
    }
    sh("ip link set lo up");
    let mut b: Child = Command::new("unshare").args(["-n", "sleep", "300"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let pid = b.id();
    let (ok, out) = sh(&format!("ip link add eA type veth peer name eB && ip link set eB netns {pid} && ip addr add 10.88.0.1/24 dev eA && ip link set eA up && nsenter -t {pid} -n sh -c 'ip link set lo up && ip addr add 10.88.0.2/24 dev eB && ip link set eB up'"));
    assert!(ok, "{out}");
    let mut listener = Command::new("python3").args(["-c", LISTEN]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));

    let now = Arc::new(AtomicI64::new(1_000_000));
    let saved = Arc::new(Mutex::new(None));
    let audit = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut e = Engine::new(Box::new(Kb), Box::new(Clk(now.clone())), Box::new(NoDns), Box::new(Mem(saved.clone())));
    let a2 = audit.clone();
    e.set_audit(Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))));
    e.set_ctx(Ctx { remote_port: Some(8443), ..Ctx::default() });
    e.boot(false);
    assert!(e.status().protected(), "the engine put its rules in the kernel at boot: {:?}", e.status());
    assert!(sh("nft list table inet iyakki_fw").0, "our table is in the real kernel");
    assert!(reach(pid), "the remote desktop port is open to the private network by the default preset");
    assert!(ping(pid), "ping from the local network is allowed");

    // a change that cuts the remote desktop off: applied for real, waiting for Keep
    let mut cut: Config = e.config().clone();
    cut.presets.clear();
    e.propose(cut.clone(), "admin").expect("propose");
    assert!(matches!(e.status(), Status::Pending { .. }), "{:?}", e.status());
    assert!(!reach(pid), "the change really cuts the remote desktop off in the kernel");
    assert!(ping(pid), "ping still works (only the preset was taken away)");
    // 30 seconds: still waiting. 61 seconds: the engine takes the change back by itself, even though the session that made it just lost its connection
    now.fetch_add(30, Ordering::SeqCst);
    e.tick();
    assert!(matches!(e.status(), Status::Pending { .. }), "after 30 s it still waits");
    assert!(!reach(pid));
    now.fetch_add(31, Ordering::SeqCst);
    e.tick();
    assert_eq!(e.status(), Status::On, "not kept within 60 s: reverted");
    assert!(reach(pid), "the old rules are back in the kernel: the remote desktop is reachable again");
    assert!(saved.lock().unwrap().is_none() || saved.lock().unwrap().as_ref().unwrap().config.presets.len() == 1, "nothing of the cut was saved");
    assert!(audit.lock().unwrap().iter().any(|l| l.starts_with("firewall.revert") && l.contains("not confirmed within 60 seconds")), "the revert is in the audit: {:?}", audit.lock().unwrap());

    // the same change, kept: it stays, and it is saved
    e.propose(cut, "admin").expect("propose again");
    assert!(!reach(pid));
    e.keep("admin").expect("keep");
    now.fetch_add(120, Ordering::SeqCst);
    e.tick();
    assert_eq!(e.status(), Status::On);
    assert!(!reach(pid), "a kept change stays in the kernel");
    assert!(saved.lock().unwrap().as_ref().is_some_and(|s| s.config.presets.is_empty()), "and it is saved");
    assert!(audit.lock().unwrap().iter().any(|l| l.starts_with("firewall.keep") && l.contains("admin")));

    // the emergency switch: our table goes, everything is reachable (no firewall), the setting is saved as off
    e.disable_now("admin").expect("emergency off");
    assert_eq!(e.status(), Status::Off);
    assert!(!sh("nft list table inet iyakki_fw").0, "the table is gone from the kernel");
    assert!(reach(pid), "with the table removed the port is reachable again");
    assert!(saved.lock().unwrap().as_ref().is_some_and(|s| !s.config.enabled), "the off state is saved");
    assert!(audit.lock().unwrap().iter().any(|l| l.starts_with("firewall.disable") && l.contains("emergency off")));
    let _ = listener.kill();
    let _ = listener.wait();
    let _ = b.kill();
    let _ = b.wait();
}
