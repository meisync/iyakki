//! IYAKKI's own FTP and FTPS client against a REAL vsftpd (ignored by default).
//!
//! Needs `vsftpd` (set IYAKKI_VSFTPD to its path, or have it on PATH; `dist/linux-host/real-tools-tests.sh` unpacks the Ubuntu package into a scratch folder when it is missing,
//! nothing is installed) and `openssl`. vsftpd runs as the normal user (so without a chroot: the test only names paths inside its scratch folder) on 127.0.0.1 with anonymous login, uploads and explicit TLS with a throw-away certificate.
//! Rows: F-FILE-02 (FTP side), F-FILE-09.

use crate::remotefs::RemoteFs;
use crate::rf_ftp;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

fn sh(cmd: &str) -> (bool, String) {
    let o = Command::new("sh").args(["-c", cmd]).output().unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

struct Ftpd {
    child: Child,
    dir: PathBuf,
    port: u16,
    files: PathBuf,
}

impl Drop for Ftpd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::env::var("IYAKKI_KEEP").is_err() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn start() -> Option<(Ftpd, String)> {
    let bin = std::env::var("IYAKKI_VSFTPD").ok().filter(|p| std::path::Path::new(p).is_file()).or_else(|| {
        let (ok, out) = sh("command -v vsftpd || ls /usr/sbin/vsftpd 2>/dev/null");
        ok.then(|| out.lines().next().unwrap_or("").trim().to_string()).filter(|p| !p.is_empty())
    });
    let Some(bin) = bin else {
        eprintln!("skipped: needs vsftpd (IYAKKI_VSFTPD=/path/to/vsftpd)");
        return None;
    };
    if !sh("command -v openssl >/dev/null").0 {
        eprintln!("skipped: needs openssl");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-vsftpd-{}", std::process::id()));
    let files = dir.join("files");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::create_dir_all(dir.join("empty")).unwrap();
    let (ok, out) = sh(&format!("cd {d} && openssl req -x509 -newkey rsa:2048 -nodes -keyout k.pem -out c.pem -days 2 -subj /CN=localhost 2>/dev/null && openssl x509 -in c.pem -noout -fingerprint -sha256", d = dir.display()));
    assert!(ok, "{out}");
    let me = String::from_utf8_lossy(&Command::new("id").arg("-un").output().unwrap().stdout).trim().to_string();
    let port = free_port();
    let d = dir.display();
    let conf = format!(
        "listen=YES\nlisten_ipv6=NO\nlisten_address=127.0.0.1\nlisten_port={port}\nbackground=NO\nanonymous_enable=YES\nlocal_enable=NO\nwrite_enable=YES\nanon_upload_enable=YES\nanon_mkdir_write_enable=YES\nanon_other_write_enable=YES\nanon_root={d}/files\nno_anon_password=YES\nrun_as_launching_user=YES\nftp_username={me}\nsecure_chroot_dir={d}/empty\npasv_enable=YES\npasv_address=127.0.0.1\nseccomp_sandbox=NO\nssl_enable=YES\nrsa_cert_file={d}/c.pem\nrsa_private_key_file={d}/k.pem\nrequire_ssl_reuse=NO\nssl_ciphers=HIGH\nallow_anon_ssl=YES\nforce_anon_logins_ssl=NO\nforce_anon_data_ssl=NO\nanon_umask=022\nvsftpd_log_file={d}/v.log\nxferlog_enable=YES\nxferlog_file={d}/xfer.log\ndual_log_enable=YES\n"
    );
    std::fs::write(dir.join("v.conf"), conf).unwrap();
    let child = Command::new(&bin).arg(dir.join("v.conf")).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("vsftpd");
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Some((Ftpd { child, dir, port, files }, out));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("vsftpd did not start");
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + i / 251) % 251) as u8).collect()
}

fn drive(tls: bool) {
    let Some((srv, _)) = start() else { return };
    let stop = || Arc::new(AtomicBool::new(false));
    let seen = std::cell::RefCell::new(String::new());
    let mut fs = rf_ftp::connect("127.0.0.1", srv.port, "anonymous", "iyakki@example.invalid", tls, &|fp| { *seen.borrow_mut() = fp.to_string(); true }, stop()).unwrap_or_else(|e| panic!("login (tls {tls}): {e}"));
    if tls {
        assert!(!seen.borrow().is_empty(), "the certificate fingerprint was offered for the check before any secret was sent");
    }
    // without root vsftpd cannot chroot: it serves the whole file system as this user, so every path below is inside the scratch folder
    let base = srv.files.display().to_string();
    let p = |n: &str| format!("{base}/{n}");
    std::fs::write(srv.files.join("a.txt"), "hello-ftp\n").unwrap();
    std::fs::create_dir(srv.files.join("sub")).unwrap();
    // list (vsftpd has no MLSD: the client falls back to LIST and parses its lines)
    let (path, mut entries) = fs.list(&base).unwrap();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(path, base);
    assert_eq!(entries.iter().map(|e| (e.name.as_str(), e.is_dir)).collect::<Vec<_>>(), vec![("a.txt", false), ("sub", true)], "tls {tls}");
    assert_eq!(entries[0].size, 10, "tls {tls}");
    fs.mkdir(&p("made")).unwrap();
    assert!(srv.files.join("made").is_dir());
    fs.rename(&p("a.txt"), &p("b.txt")).unwrap();
    assert!(srv.files.join("b.txt").exists() && !srv.files.join("a.txt").exists());
    assert_eq!(fs.size(&p("b.txt")).unwrap(), Some(10));
    assert_eq!(fs.size(&p("nothing")).unwrap(), None);
    fs.remove_file(&p("b.txt")).unwrap();
    fs.remove_dir(&p("made")).unwrap();
    // a file bigger than a few buffers, up and down, with a resume in both directions (REST for the download, APPE or REST for the upload)
    let big = pattern(3 * 1024 * 1024 + 77);
    let first = &big[..1024 * 1024];
    fs.upload(&p("up.bin"), 0, &mut &first[..], &mut |_| true).unwrap();
    assert_eq!(fs.size(&p("up.bin")).unwrap(), Some(first.len() as u64), "tls {tls}");
    let mut rest = &big[1024 * 1024..];
    fs.upload(&p("up.bin"), first.len() as u64, &mut rest, &mut |_| true).unwrap();
    assert_eq!(std::fs::read(srv.files.join("up.bin")).unwrap(), big, "the uploaded file is identical (tls {tls})");
    let mut got = Vec::new();
    fs.download(&p("up.bin"), 0, &mut got, &mut |_| true).unwrap();
    assert_eq!(got, big, "the downloaded file is identical (tls {tls})");
    let mut tail = Vec::new();
    fs.download(&p("up.bin"), 2 * 1024 * 1024, &mut tail, &mut |_| true).unwrap();
    assert_eq!(tail, big[2 * 1024 * 1024..], "a resumed download is identical (tls {tls})");
    assert!(fs.list(&p("nope")).is_err());
    // the certificate check: a client that does not trust the fingerprint ends before any secret is sent
    if tls {
        let e = rf_ftp::connect("127.0.0.1", srv.port, "anonymous", "x", true, &|_| false, stop()).err().expect("an untrusted certificate ends the connection");
        assert!(!e.to_string().is_empty());
    }
}

#[test]
#[ignore]
fn real_vsftpd_plain_ftp_with_our_own_client() {
    drive(false);
}

#[test]
#[ignore]
fn real_vsftpd_explicit_tls_ftps_with_our_own_client() {
    drive(true);
}
