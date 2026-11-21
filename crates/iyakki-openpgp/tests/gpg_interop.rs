//! Interop with the real GnuPG: keys and signatures made by `gpg` itself at test time (not stored fixtures),
//! checked by the in-house verifier. Needs `gpg` on the PATH; a machine without it fails loudly rather than passing.
use iyakki_openpgp::verify_detached;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Home(PathBuf);
impl Home {
    fn new(tag: &str) -> Home {
        let p = std::env::temp_dir().join(format!("iyakki-gpg-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        // gpg refuses a home with open permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        Home(p)
    }
    fn gpg(&self, args: &[&str]) -> Vec<u8> {
        let o = Command::new("gpg")
            .arg("--homedir").arg(&self.0).args(["--batch", "--yes", "--no-tty", "--pinentry-mode", "loopback", "--passphrase", ""])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("gpg must be installed for the interop test");
        assert!(o.status.success(), "gpg {:?} failed: {}", args, String::from_utf8_lossy(&o.stderr));
        o.stdout
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = Command::new("gpgconf").arg("--homedir").arg(&self.0).args(["--kill", "all"]).output();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn payload(dir: &Path) -> (PathBuf, Vec<u8>) {
    let data: Vec<u8> = (0..70_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let f = dir.join("payload.bin");
    std::fs::write(&f, &data).unwrap();
    (f, data)
}

fn make_key(h: &Home, uid: &str, algo: &str) {
    h.gpg(&["--quick-generate-key", uid, algo, "sign", "never"]);
}

#[test]
fn signatures_made_by_gnupg_verify_for_every_hash_and_both_encodings() {
    let h = Home::new("hashes");
    make_key(&h, "interop-a@example.invalid", "rsa2048");
    let key_asc = h.gpg(&["--armor", "--export", "interop-a@example.invalid"]);
    let key_bin = h.gpg(&["--export", "interop-a@example.invalid"]);
    assert!(key_asc.starts_with(b"-----BEGIN PGP PUBLIC KEY BLOCK-----"));
    let (file, data) = payload(&h.0);
    for hash in ["SHA256", "SHA384", "SHA512"] {
        for armor in [false, true] {
            let mut args = vec!["--local-user", "interop-a@example.invalid", "--digest-algo", hash, "--detach-sign", "--output", "-"];
            if armor {
                args.insert(0, "--armor");
            }
            args.push(file.to_str().unwrap());
            let sig = h.gpg(&args);
            for key in [&key_asc, &key_bin] {
                verify_detached(key, &sig, &data).unwrap_or_else(|e| panic!("{hash} armor={armor}: {e}"));
                let mut bad = data.clone();
                bad[40_000] ^= 0x10;
                assert!(verify_detached(key, &sig, &bad).is_err(), "{hash} armor={armor}: changed file accepted");
            }
        }
    }
}

#[test]
fn a_bigger_key_and_a_signing_subkey_verify_and_another_gnupg_key_does_not() {
    let h = Home::new("subkey");
    // primary key certifies only; the signature is made by a signing subkey
    h.gpg(&["--quick-generate-key", "interop-b@example.invalid", "rsa3072", "cert", "never"]);
    let fpr = String::from_utf8(h.gpg(&["--with-colons", "--list-keys", "interop-b@example.invalid"])).unwrap();
    let fpr = fpr.lines().find(|l| l.starts_with("fpr:")).unwrap().split(':').nth(9).unwrap().to_string();
    h.gpg(&["--quick-add-key", &fpr, "rsa3072", "sign", "never"]);
    make_key(&h, "interop-c@example.invalid", "rsa2048");
    let key_b = h.gpg(&["--armor", "--export", "interop-b@example.invalid"]);
    let key_c = h.gpg(&["--armor", "--export", "interop-c@example.invalid"]);
    let (file, data) = payload(&h.0);
    let sig = h.gpg(&["--local-user", "interop-b@example.invalid", "--detach-sign", "--output", "-", file.to_str().unwrap()]);
    verify_detached(&key_b, &sig, &data).expect("a signature by the signing subkey verifies against the exported key");
    assert!(verify_detached(&key_c, &sig, &data).is_err(), "another key must not verify");
    // and the real gpg agrees on the same facts, so the test cannot drift from the reference
    let sigf = h.0.join("payload.sig");
    std::fs::write(&sigf, &sig).unwrap();
    h.gpg(&["--verify", sigf.to_str().unwrap(), file.to_str().unwrap()]);
}
