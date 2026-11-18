//! A real OpenSSH `sftp` client against the in-tree SSH server (ignored by default: it needs `sftp` and `ssh-keygen` on the machine).
//! Run: `cargo test -p iyakki-sshfs -- --ignored real_openssh`. For each cipher the server offers it logs in with a key, lists, downloads and
//! uploads a 3 MB file and compares every byte; the server re-keys every 500 000 bytes, so the client's answer to a server-started re-key is
//! checked too. F-FILE-12 records the run.
#[cfg(test)]
mod interop {
    use crate::sftp_session::Session;
    use crate::ssh_server::*;
    use crate::sshkey::KeyPair;
    use std::net::{IpAddr, TcpListener};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

struct G(Mutex<Vec<crate::sftp_fs::Jail>>, Arc<Mutex<Vec<String>>>);
impl Gate for G {
    fn key_ok(&self, _: &str, _: &[u8], _: IpAddr) -> bool { true }
    fn password_ok(&self, _: &str, _: &str, _: IpAddr) -> bool { false }
    fn session(&self, _: &str, _: IpAddr) -> Option<Session> { let j = self.0.lock().unwrap().pop()?; Some(Session::new(j, Box::new(|_, _| {}))) }
    fn audit(&self, e: &str, d: &str) { self.1.lock().unwrap().push(format!("{e} {d}")); }
}

#[test]
#[ignore]
fn real_openssh_sftp_with_each_cipher() {
    let (w, _) = crate::testworld::world("interop");
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
    std::fs::write(w.home.join("big.bin"), &big).unwrap();
    let kd = w.root.join("k");
    std::fs::create_dir_all(&kd).unwrap();
    let st = std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(kd.join("id")).status().unwrap();
    assert!(st.success());
    for cipher in ["chacha20-poly1305@openssh.com", "aes256-gcm@openssh.com", "aes256-ctr"] {
        let (_w2, jail) = crate::testworld::world(&format!("interop-{}", &cipher[..6]));
        std::fs::write(_w2.home.join("big.bin"), &big).unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(G(Mutex::new(vec![jail]), log.clone()));
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let g2 = gate.clone();
        let th = std::thread::spawn(move || {
            let (s, a) = l.accept().unwrap();
            prepare(&s).unwrap();
            let key = KeyPair::from_seed([1; 32], "h");
            let srv = Server { host_key: &key, gate: &*g2, ec_host: None, peer: a.ip() };
            serve_with(s, &srv, Arc::new(AtomicBool::new(false)), 500_000)
        });
        let out = std::process::Command::new("sftp")
            .args(["-P", &port.to_string(), "-i"]).arg(kd.join("id"))
            .args(["-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "BatchMode=yes", "-o", &format!("Ciphers={cipher}"), "-o", "PreferredAuthentications=publickey", "-v"])
            .arg("-b").arg("-").arg("amy@127.0.0.1")
            .current_dir(&kd)
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
            .spawn().unwrap();
        use std::io::Write;
        let mut out = out;
        out.stdin.take().unwrap().write_all(b"ls\nget big.bin\nput big.bin up.bin\nls\n").unwrap();
        let o = out.wait_with_output().unwrap();
        let err = String::from_utf8_lossy(&o.stderr);
        let rk = err.matches("rekey").count();
        eprintln!("{cipher}: status {:?}, stdout {} bytes, rekey lines {rk}, audit {:?}", o.status, o.stdout.len(), log.lock().unwrap());
        assert!(o.status.success(), "{cipher}: {err}");
        let got = std::fs::read(kd.join("big.bin")).unwrap();
        assert_eq!(got, big, "{cipher} download");
        assert_eq!(std::fs::read(_w2.home.join("up.bin")).unwrap(), big, "{cipher} upload");
        assert!(err.contains(&format!("cipher: {cipher}")) || err.contains(cipher), "{cipher}: cipher not used? {err}");
        let _ = th.join();
        let _ = std::fs::remove_file(kd.join("big.bin"));
    }
}


    /// RSA (2048 and 4096 bits) and ECDSA P-256 login keys made by `ssh-keygen`, an RSA key that is too small (refused), and the ECDSA host
    /// key (the client is told to accept nothing else).
    #[test]
    #[ignore]
    fn real_openssh_logs_in_with_rsa_and_ecdsa_keys_and_accepts_the_ecdsa_host_key() {
        let (w, _) = crate::testworld::world("interop-keys");
        let kd = w.root.join("k");
        std::fs::create_dir_all(&kd).unwrap();
        let ec = crate::sshalg::EcHostKey::generate().unwrap();
        let cases: [(&str, &[&str], bool); 5] = [("rsa2048", &["-t", "rsa", "-b", "2048"], true), ("rsa4096", &["-t", "rsa", "-b", "4096"], true), ("ecdsa256", &["-t", "ecdsa", "-b", "256"], true), ("ecdsa384", &["-t", "ecdsa", "-b", "384"], true), ("rsa1024", &["-t", "rsa", "-b", "1024"], false)];
        for (name, kg, should_work) in cases {
            let id = kd.join(name);
            let st = std::process::Command::new("ssh-keygen").args(["-q", "-N", "", "-f"]).arg(&id).args(kg).status().unwrap();
            assert!(st.success(), "{name}");
            for host_alg in ["ssh-ed25519", "ecdsa-sha2-nistp256"] {
                let (_w2, jail) = crate::testworld::world(&format!("interop-k-{name}-{}", &host_alg[..5]));
                let log = Arc::new(Mutex::new(Vec::new()));
                let gate = Arc::new(G(Mutex::new(vec![jail]), log.clone()));
                let l = TcpListener::bind("127.0.0.1:0").unwrap();
                let port = l.local_addr().unwrap().port();
                let (g2, ec2) = (gate.clone(), ec.to_pkcs8().to_vec());
                let th = std::thread::spawn(move || {
                    let (s, a) = l.accept().unwrap();
                    prepare(&s).unwrap();
                    let key = KeyPair::from_seed([1; 32], "h");
                    let ec = crate::sshalg::EcHostKey::from_pkcs8(&ec2).unwrap();
                    let srv = Server { host_key: &key, ec_host: Some(&ec), gate: &*g2, peer: a.ip() };
                    serve(s, &srv, Arc::new(AtomicBool::new(false)))
                });
                let mut cmd = std::process::Command::new("sftp");
                cmd.args(["-P", &port.to_string(), "-i"]).arg(&id).args(["-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes", "-o", "PreferredAuthentications=publickey", "-o", &format!("HostKeyAlgorithms={host_alg}"), "-v", "-b", "-", "amy@127.0.0.1"]);
                cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
                let mut child = cmd.spawn().unwrap();
                use std::io::Write;
                child.stdin.take().unwrap().write_all(b"ls\n").unwrap();
                let o = child.wait_with_output().unwrap();
                let err = String::from_utf8_lossy(&o.stderr);
                eprintln!("{name} host {host_alg}: success={} audit={:?}", o.status.success(), log.lock().unwrap());
                assert_eq!(o.status.success(), should_work, "{name} {host_alg}: {err}");
                if should_work {
                    assert!(err.contains(&format!("Server host key: {host_alg}")), "{name} {host_alg}: {err}");
                }
                let _ = th.join();
            }
        }
    }
}
