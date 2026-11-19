//! IYAKKI's own SSH and SFTP client against a REAL OpenSSH `sshd` (ignored by default).
//!
//! Needs `sshd`, `ssh-keygen` and `sshd` able to run as the normal user (it listens on 127.0.0.1 on a free port with its own host key, its own
//! `authorized_keys` and `PasswordAuthentication no`; nothing in /etc is read or written). Run: `cargo test -p iyakki-sshfs -- --ignored real_sshd`
//! (`dist/linux-host/real-tools-tests.sh`). The server reaches the whole file system as the user, so the test only names files inside its own scratch folder.
//! Rows: F-FILE-02, F-FILE-08.

use crate::remotefs::RemoteFs;
use crate::rf_sftp::SftpFs;
use crate::rf_ssh;
use crate::sshkey::KeyPair;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

fn sh(cmd: &str) -> (bool, String) {
    let o = Command::new("sh").args(["-c", &format!("export PATH=$PATH:/usr/sbin:/sbin; {cmd}")]).output().unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

struct Sshd {
    child: Child,
    port: u16,
    dir: PathBuf,
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn free_port() -> u16 {
    crate::testdir::free_port()
}

fn start_sshd(tag: &str, authorized: &str) -> Option<(Sshd, String)> {
    if !(sh("command -v sshd >/dev/null").0 && sh("command -v ssh-keygen >/dev/null").0) {
        eprintln!("skipped: needs sshd and ssh-keygen");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-sshd-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port = free_port();
    let (ok, out) = sh(&format!("cd {d} && ssh-keygen -q -t ed25519 -N '' -f hostkey && ssh-keygen -lf hostkey.pub", d = dir.display()));
    assert!(ok, "{out}");
    let host_fp = out.split_whitespace().nth(1).unwrap().to_string();
    std::fs::write(dir.join("authorized_keys"), authorized).unwrap();
    let conf = format!(
        "Port {port}\nListenAddress 127.0.0.1\nHostKey {d}/hostkey\nPidFile {d}/sshd.pid\nAuthorizedKeysFile {d}/authorized_keys\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nStrictModes no\nSubsystem sftp internal-sftp\nLogLevel VERBOSE\n",
        d = dir.display()
    );
    std::fs::write(dir.join("sshd_config"), conf).unwrap();
    let child = Command::new("/usr/sbin/sshd").args(["-D", "-e", "-f"]).arg(dir.join("sshd_config")).stdout(Stdio::null()).stderr(Stdio::from(std::fs::File::create(dir.join("sshd.log")).unwrap())).spawn().expect("sshd");
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Some((Sshd { child, port, dir }, host_fp));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("sshd did not start: {}", std::fs::read_to_string(dir.join("sshd.log")).unwrap_or_default());
}

fn tcp(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    s
}

fn user() -> String {
    String::from_utf8_lossy(&Command::new("id").arg("-un").output().unwrap().stdout).trim().to_string()
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + i / 251) % 251) as u8).collect()
}

#[test]
#[ignore]
fn real_sshd_real_openssh_server_with_our_own_ssh_and_sftp_client() {
    // a client key made by ssh-keygen (OpenSSH format) and read by our own parser
    let kdir = std::env::temp_dir().join(format!("iyakki-sshd-keys-{}", std::process::id()));
    std::fs::create_dir_all(&kdir).unwrap();
    if !sh("command -v ssh-keygen >/dev/null").0 {
        eprintln!("skipped: needs ssh-keygen");
        return;
    }
    let (ok, out) = sh(&format!("ssh-keygen -q -t ed25519 -N '' -f {k}/id && ssh-keygen -q -t ed25519 -N '' -f {k}/other", k = kdir.display()));
    assert!(ok, "{out}");
    let key = KeyPair::from_openssh(&std::fs::read_to_string(kdir.join("id")).unwrap(), None).expect("our parser reads ssh-keygen's private key");
    let other = KeyPair::from_openssh(&std::fs::read_to_string(kdir.join("other")).unwrap(), None).unwrap();
    let authorized = std::fs::read_to_string(kdir.join("id.pub")).unwrap();
    let Some((srv, host_fp)) = start_sshd("a", &authorized) else { return };
    let me = user();
    let stop = || Arc::new(AtomicBool::new(false));
    let scratch: PathBuf = srv.dir.join("files");
    std::fs::create_dir_all(&scratch).unwrap();
    let p = |name: &str| format!("{}/{name}", scratch.display());

    // the host key check: the right fingerprint is accepted (and our fingerprint format equals ssh-keygen's), another one is refused
    let fp_seen = std::cell::RefCell::new(String::new());
    let ssh = rf_ssh::connect(tcp(srv.port), &me, None, Some(&key), &|_, f| { *fp_seen.borrow_mut() = f.to_string(); f == host_fp }, stop()).unwrap_or_else(|e| panic!("login with the key: {e}"));
    assert_eq!(*fp_seen.borrow(), host_fp, "the host key fingerprint equals `ssh-keygen -lf`");
    let mut fs = SftpFs::start(ssh).expect("the sftp subsystem of OpenSSH starts");

    // list, make, move, remove
    std::fs::write(scratch.join("a.txt"), "alpha").unwrap();
    std::fs::create_dir(scratch.join("sub")).unwrap();
    let (path, mut entries) = fs.list(&scratch.display().to_string()).unwrap();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(path, scratch.display().to_string());
    assert_eq!(entries.iter().map(|e| (e.name.as_str(), e.is_dir)).collect::<Vec<_>>(), vec![("a.txt", false), ("sub", true)]);
    assert_eq!(entries[0].size, 5);
    fs.mkdir(&p("made")).unwrap();
    assert!(scratch.join("made").is_dir() && fs.mkdir(&p("made")).is_err());
    fs.rename(&p("a.txt"), &p("b.txt")).unwrap();
    assert!(scratch.join("b.txt").exists() && !scratch.join("a.txt").exists());
    assert_eq!(fs.size(&p("b.txt")).unwrap(), Some(5));
    assert_eq!(fs.size(&p("nothing")).unwrap(), None);
    fs.remove_file(&p("b.txt")).unwrap();
    fs.remove_dir(&p("made")).unwrap();
    assert!(fs.list(&p("nope")).is_err());

    // a file larger than the SSH window, up and down, byte for byte, with a resume in each direction (this also crosses OpenSSH's re-key limits of data)
    let big = pattern(5 * 1024 * 1024 + 123);
    let first = &big[..1024 * 1024];
    let mut seen = 0u64;
    fs.upload(&p("up.bin"), 0, &mut &first[..], &mut |n| { seen += n; true }).unwrap();
    assert_eq!((seen, fs.size(&p("up.bin")).unwrap()), (first.len() as u64, Some(first.len() as u64)));
    let mut rest = &big[1024 * 1024..];
    fs.upload(&p("up.bin"), first.len() as u64, &mut rest, &mut |_| true).unwrap();
    assert_eq!(std::fs::read(scratch.join("up.bin")).unwrap(), big, "the uploaded file is identical");
    let mut got = Vec::new();
    fs.download(&p("up.bin"), 0, &mut got, &mut |_| true).unwrap();
    assert_eq!(got, big, "the downloaded file is identical");
    let mut tail = Vec::new();
    fs.download(&p("up.bin"), 4 * 1024 * 1024, &mut tail, &mut |_| true).unwrap();
    assert_eq!(tail, big[4 * 1024 * 1024..], "a resumed download is identical");
    // the permissions of a file a person can read in the listing
    let (_, ents) = fs.list(&scratch.display().to_string()).unwrap();
    let e = ents.iter().find(|e| e.name == "up.bin").unwrap();
    assert!(e.modified > 1_700_000_000 && e.perms.len() == 9 && e.size == big.len() as u64, "{e:?}");

    // refusals: another key, no key, a changed host key
    let e = rf_ssh::connect(tcp(srv.port), &me, None, Some(&other), &|_, f| f == host_fp, stop()).err().expect("an unknown key is refused");
    assert!(!e.to_string().is_empty());
    let e = rf_ssh::connect(tcp(srv.port), &me, Some("password"), None, &|_, f| f == host_fp, stop()).err().expect("a password is refused when the server has none");
    assert!(!e.to_string().is_empty());
    let e = rf_ssh::connect(tcp(srv.port), &me, None, Some(&key), &|_, _| false, stop()).err().expect("a host key that is not trusted ends the connection");
    assert!(!e.to_string().is_empty());
    let log = std::fs::read_to_string(srv.dir.join("sshd.log")).unwrap_or_default();
    assert!(log.contains("Accepted publickey"), "sshd's own log shows our login: {log}");
    let _ = std::fs::remove_dir_all(&kdir);
}
