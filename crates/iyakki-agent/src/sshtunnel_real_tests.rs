//! SSH tunnels against a real OpenSSH server: the system `sshd` started by the test, as the current user, on a high port of 127.0.0.1
//! in a temporary folder (own host key, own authorized key, no PAM), and the real `OpenSshRunner` with the arguments of
//! `build_args`, the in-memory agent and a known-hosts file. A byte goes through the local forward to a small echo server and back.
//! The tests skip (and say so) when `sshd`, `ssh` or `ssh-keygen` is not installed or the server cannot start here.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::sshkey::KeyPair;
use crate::sshtunnel::*;
use crate::sshtunnel_mgr::*;

pub(crate) fn find(name: &str) -> Option<PathBuf> {
    ["/usr/sbin", "/usr/bin", "/usr/local/sbin", "/usr/local/bin", "/opt/homebrew/bin", "/bin", "/sbin"].iter().map(|d| Path::new(d).join(name)).find(|p| p.is_file())
}

pub(crate) fn temp(tag: &str) -> PathBuf {
    // the key socket path must stay under 100 bytes: use the temp folder only when it is short, else /tmp
    let base = std::env::temp_dir();
    let base = if base.as_os_str().len() <= 40 { base } else { PathBuf::from("/tmp") };
    let d = base.join(format!("iyakki-sr-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

pub(crate) struct Server {
    child: Child,
    pub(crate) port: u16,
    pub(crate) user: String,
    pub(crate) host_pub: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A user-run sshd that accepts exactly `authorized` for the current user; `None` (with a reason on stderr) where that is not possible.
pub(crate) fn start_sshd(dir: &Path, authorized: &str) -> Option<Server> {
    let (sshd, keygen) = (find("sshd")?, find("ssh-keygen")?);
    let host = dir.join("host_key");
    let ok = Command::new(keygen).args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&host).status().ok()?.success();
    if !ok {
        return None;
    }
    let host_pub = std::fs::read_to_string(host.with_extension("pub")).ok()?;
    let host_pub = host_pub.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    let auth = dir.join("authorized_keys");
    std::fs::write(&auth, format!("{authorized}\n")).ok()?;
    let user = Command::new("id").arg("-un").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|u| !u.is_empty())?;
    let port = OsPorts.pick().ok()?;
    let cfg = dir.join("sshd_config");
    std::fs::write(
        &cfg,
        format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nAuthorizedKeysFile {}\nPidFile {}\nUsePAM no\nStrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nAllowTcpForwarding yes\nPermitTTY no\nLogLevel ERROR\nAllowUsers {user}\n",
            host.display(),
            auth.display(),
            dir.join("sshd.pid").display()
        ),
    )
    .ok()?;
    let child = Command::new(sshd).args(["-D", "-e", "-f"]).arg(&cfg).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?;
    let mut s = Server { child, port, user, host_pub };
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(5) {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Some(s);
        }
        if s.child.try_wait().ok().flatten().is_some() {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

fn echo_server() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut s = s;
            let mut buf = [0u8; 64];
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 || s.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    });
    port
}

fn tunnel(srv: &Server, local: u16, target: u16) -> Tunnel {
    Tunnel { id: "t1".into(), name: "real".into(), host: "127.0.0.1".into(), port: srv.port, user: srv.user.clone(), key: "k".into(), kind: Kind::LocalForward, local_port: local, target_host: "127.0.0.1".into(), target_port: target, enabled: true, ..Default::default() }
}

fn runner(dir: &Path) -> OpenSshRunner {
    OpenSshRunner { search: std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()), dir: dir.to_path_buf(), ports: Arc::new(OsPorts), audit: Arc::new(|_, _| {}) }
}

fn wait_listening(p: &mut Box<dyn Proc>) -> Result<(), String> {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(20) {
        match p.poll() {
            ProcState::Running { listening: true } => return Ok(()),
            ProcState::Running { .. } => std::thread::sleep(Duration::from_millis(100)),
            ProcState::Exited { stderr } => return Err(stderr),
        }
    }
    Err("never listened".into())
}

#[test]
fn a_local_forward_through_a_real_openssh_server_carries_bytes_with_the_in_memory_key() {
    if find("ssh").is_none() {
        eprintln!("SKIP: no ssh client here");
        return;
    }
    let dir = temp("fwd");
    let kp = KeyPair::generate("n1c").unwrap();
    let Some(srv) = start_sshd(&dir, &kp.public_line()) else {
        eprintln!("SKIP: no sshd, or it cannot be started here as this user");
        return;
    };
    let target = echo_server();
    let local = OsPorts.pick().unwrap();
    let t = tunnel(&srv, local, target);
    let kh = dir.join("known_hosts");
    std::fs::write(&kh, format!("[127.0.0.1]:{} {}\n", srv.port, srv.host_pub)).unwrap();
    let args = build_args(&t, &kh).unwrap();
    let mut proc = runner(&dir).start(&Launch { id: t.id.clone(), args, local_port: local, key: kp, key_name: "k".into() }).unwrap();
    wait_listening(&mut proc).unwrap_or_else(|e| panic!("the client did not come up: {e}"));
    let mut c = std::net::TcpStream::connect(("127.0.0.1", local)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.write_all(b"ping").unwrap();
    let mut got = [0u8; 4];
    c.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"ping", "the bytes went through the tunnel and back");
    proc.kill();
    assert!(matches!(proc.poll(), ProcState::Exited { .. }), "a killed client is gone");
}

#[test]
fn a_changed_host_key_or_a_key_the_server_does_not_know_ends_the_client_with_a_classified_error() {
    if find("ssh").is_none() {
        eprintln!("SKIP: no ssh client here");
        return;
    }
    let dir = temp("bad");
    let kp = KeyPair::generate("n1c").unwrap();
    let Some(srv) = start_sshd(&dir, &kp.public_line()) else {
        eprintln!("SKIP: no sshd, or it cannot be started here as this user");
        return;
    };
    let target = echo_server();
    // (1) a known-hosts file that holds another host key for this server
    let other = KeyPair::generate("other").unwrap();
    let kh = dir.join("known_hosts");
    std::fs::write(&kh, format!("[127.0.0.1]:{} {}\n", srv.port, other.public_line().split_whitespace().take(2).collect::<Vec<_>>().join(" "))).unwrap();
    let local = OsPorts.pick().unwrap();
    let t = tunnel(&srv, local, target);
    let args = build_args(&t, &kh).unwrap();
    let mut proc = runner(&dir).start(&Launch { id: t.id.clone(), args, local_port: local, key: kp.clone(), key_name: "k".into() }).unwrap();
    let err = wait_listening(&mut proc).unwrap_err();
    assert_ne!(classify(&err), Fault::Other, "the host-key refusal has its own words: {err}");
    // (2) the right host key but a client key the server does not hold
    std::fs::write(&kh, format!("[127.0.0.1]:{} {}\n", srv.port, srv.host_pub)).unwrap();
    let stranger = KeyPair::generate("stranger").unwrap();
    let local2 = OsPorts.pick().unwrap();
    let t2 = tunnel(&srv, local2, target);
    let args = build_args(&t2, &kh).unwrap();
    let mut proc = runner(&dir).start(&Launch { id: "t2".into(), args, local_port: local2, key: stranger, key_name: "k".into() }).unwrap();
    let err = wait_listening(&mut proc).unwrap_err();
    assert_ne!(classify(&err), Fault::Other, "a refused key has its own words: {err}");
}
