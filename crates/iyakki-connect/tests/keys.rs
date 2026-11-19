//! `iyakki-connect key ...`: the device key file (mode 600, never printed, never replaced by accident), the OpenSSH import (ed25519 only,
//! no RSA, no passphrase-protected keys), and the checks `serve` makes on its arguments before it listens.

use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use iyakki_connect::{pins::Store, run, serve_start, Ctx, Fail};

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-keys-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn ctx(d: &std::path::Path) -> Ctx {
    Ctx { store: Store::new(d), timeout: Duration::from_secs(2), interactive: false }
}

fn go(c: &Ctx, args: &[&str]) -> Result<String, Fail> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    run(&args, c, &mut Cursor::new(Vec::new()), &mut out).map(|_| String::from_utf8(out).unwrap())
}

const SSH_PRIVATE: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACDBFdOcQethiwbFQaclY0v+gI6QoIOpKg7POcHd3uzySAAAAJhf8C2gX/At\noAAAAAtzc2gtZWQyNTUxOQAAACDBFdOcQethiwbFQaclY0v+gI6QoIOpKg7POcHd3uzySA\nAAAEDjAjfXJY3fPBVR0hh6MpgtJ8FtWg7JjdManPOQy5mVicEV05xB62GLBsVBpyVjS/6A\njpCgg6kqDs85wd3e7PJIAAAAD2l5YWtraS10ZXN0LWtleQECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";
const SSH_PUBLIC: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMEV05xB62GLBsVBpyVjS/6AjpCgg6kqDs85wd3e7PJI iyakki-test-key\n";

#[test]
fn a_new_key_is_private_shown_as_an_ssh_line_and_never_replaced_by_accident() {
    let d = dir("new");
    let c = ctx(&d);
    assert!(go(&c, &["key", "show"]).unwrap_err().text().contains("no device key"));
    let out = go(&c, &["key", "new"]).unwrap();
    assert!(out.contains("ssh-ed25519 AAAA") && out.contains("fingerprint: SHA256:"), "{out}");
    let file = d.join("device-key");
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700);
    let seed = std::fs::read_to_string(&file).unwrap().split_whitespace().nth(1).unwrap().to_string();
    assert_eq!(seed.len(), 64);
    assert!(!out.contains(&seed), "the secret is never printed");
    let shown = go(&c, &["key", "show"]).unwrap();
    assert!(!shown.contains(&seed));
    assert!(shown.starts_with(out.lines().nth(1).unwrap()), "show prints the same public line");
    // a second `new` does not destroy the first
    assert!(go(&c, &["key", "new"]).unwrap_err().text().contains("--force"));
    assert!(std::fs::read_to_string(&file).unwrap().contains(&seed));
    go(&c, &["key", "new", "--force"]).unwrap();
    assert!(!std::fs::read_to_string(&file).unwrap().contains(&seed), "forced: replaced");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_key_file_others_can_read_is_not_used() {
    let d = dir("mode");
    let c = ctx(&d);
    go(&c, &["key", "new"]).unwrap();
    std::fs::set_permissions(d.join("device-key"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let e = go(&c, &["key", "show"]).unwrap_err();
    assert!(e.text().contains("can be read by other people"), "{}", e.text());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn an_openssh_ed25519_key_is_imported_and_its_public_half_is_the_known_one() {
    let d = dir("import");
    let c = ctx(&d);
    let k = d.join("id_ed25519");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(&k, SSH_PRIVATE).unwrap();
    let out = go(&c, &["key", "import-ssh", k.to_str().unwrap()]).unwrap();
    let want = SSH_PUBLIC.split_whitespace().nth(1).unwrap();
    assert!(out.contains(want), "the imported key's public half is the file's: {out}");
    assert_eq!(std::fs::metadata(d.join("device-key")).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(go(&c, &["key", "import-ssh", k.to_str().unwrap()]).unwrap_err().text().contains("--force"), "no silent replacement");
    // a public key file is explained, not accepted
    let p = d.join("id_ed25519.pub");
    std::fs::write(&p, SSH_PUBLIC).unwrap();
    let e = go(&c, &["key", "import-ssh", p.to_str().unwrap(), "--force"]).unwrap_err();
    assert!(e.text().contains("public key") && e.text().contains("private"), "{}", e.text());
    // RSA, garbage and a missing file
    let rsa = d.join("id_rsa");
    std::fs::write(&rsa, "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----\n").unwrap();
    assert!(go(&c, &["key", "import-ssh", rsa.to_str().unwrap(), "--force"]).is_err());
    assert!(go(&c, &["key", "import-ssh", d.join("nothing").to_str().unwrap(), "--force"]).is_err());
    assert!(go(&c, &["key", "show"]).unwrap().contains(want), "a failed import leaves the key as it was");
    let _ = std::fs::remove_dir_all(d);
}

fn serve_err(c: &Ctx, args: &[&str]) -> String {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    match serve_start(&args, c, &mut Cursor::new(b"pw\n".to_vec())) {
        Ok(_) => panic!("started"),
        Err(f) => f.text().to_string(),
    }
}

#[test]
fn serve_checks_its_arguments_before_it_listens() {
    let d = dir("args");
    let c = ctx(&d);
    assert!(serve_err(&c, &["serve", "acme.example:1", "--user", "ann"]).contains("not pinned"));
    assert!(serve_err(&c, &["serve", "acme.example:1"]).contains("--user"));
    assert!(serve_err(&c, &["serve"]).contains("usage"));
    c.store.set("acme.example:1", iyakki_link::cloudclient::Pin([1; 32])).unwrap();
    assert!(serve_err(&c, &["serve", "acme.example:1", "--user", "ann", "--write"]).contains("--share"));
    assert!(serve_err(&c, &["serve", "acme.example:1", "--user", "ann", "--key"]).contains("no device key"));
    go(&c, &["key", "new"]).unwrap();
    assert!(serve_err(&c, &["serve", "acme.example:1", "--user", "ann", "--key", "--share", d.to_str().unwrap()]).contains("password"));
    assert!(serve_err(&c, &["serve", "acme.example:1", "--user", "ann", "--share", "/nonexistent-iyakki-dir"]).contains("nonexistent"));
    assert!(serve_err(&c, &["serve", "acme.example:1", "--user", "ann", "--port", "99999"]).contains("not a port"));
    let args: Vec<String> = ["serve", "acme.example:1", "--user", "ann"].iter().map(|s| s.to_string()).collect();
    let e = serve_start(&args, &c, &mut Cursor::new(b"\n".to_vec())).err().unwrap();
    assert!(e.text().contains("no password"));
    let _ = std::fs::remove_dir_all(d);
}
