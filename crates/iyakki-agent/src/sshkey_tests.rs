use super::*;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("-?").output().is_ok()
}

fn temp(tag: &str) -> std::path::PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 4];
    iyakki_sec::random_fill(&mut r).unwrap();
    let d = std::path::Path::new(&base).join(format!("sshkey-{tag}-{:02x}{:02x}{:02x}{:02x}", r[0], r[1], r[2], r[3]));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_blowfish_tables_computed_from_pi_match_the_known_first_and_last_words() {
    let w = pi_words();
    assert_eq!(w[0], 0x243F6A88);
    assert_eq!(w[1], 0x85A308D3);
    assert_eq!(w[17], 0x8979FB1B); // P[17]
    assert_eq!(w[18], 0xD1310BA6); // S0[0]
    assert_eq!(w[18 + 255], 0x6E85076A); // S0[255]
    assert_eq!(w[1041], 0x3AC372E6); // S3[255]
}

#[test]
fn a_generated_key_round_trips_through_the_openssh_format_and_signs() {
    let k = KeyPair::generate("me@host").unwrap();
    let text = k.to_openssh();
    assert!(text.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n") && text.trim_end().ends_with("-----END OPENSSH PRIVATE KEY-----"));
    assert!(text.lines().all(|l| l.len() <= 70 || l.starts_with("-----")));
    assert!(!is_encrypted(&text));
    let back = KeyPair::from_openssh(&text, None).unwrap();
    assert_eq!(back.public_bytes(), k.public_bytes());
    assert_eq!(back.comment, "me@host");
    assert_eq!(public_line_of(&text).unwrap(), format!("ssh-ed25519 {}", b64(&k.public_blob())));
    assert!(k.public_line().starts_with("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5") && k.public_line().ends_with(" me@host"));
    let wire = k.sign_wire(b"hello");
    let mut r = Reader { b: &wire };
    assert_eq!(r.string().unwrap(), b"ssh-ed25519");
    let sig = r.string().unwrap();
    let vk = VerifyingKey::from_bytes(&k.public_bytes()).unwrap();
    vk.verify(b"hello", &Signature::from_slice(sig).unwrap()).unwrap();
    assert!(vk.verify(b"hellp", &Signature::from_slice(sig).unwrap()).is_err());
    assert!(k.fingerprint().starts_with("SHA256:") && !k.fingerprint().contains('='));
    assert!(!format!("{k:?}").contains(&b64(&k.seed[..])));
}

#[test]
fn garbage_and_other_key_types_are_refused_plainly() {
    assert_eq!(KeyPair::from_openssh("hello", None).unwrap_err(), KeyError::NotAKey);
    assert!(matches!(KeyPair::from_openssh("-----BEGIN OPENSSH PRIVATE KEY-----\n!!!\n-----END OPENSSH PRIVATE KEY-----", None), Err(KeyError::Damaged(_))));
    let k = KeyPair::generate("x").unwrap();
    let cut: String = k.to_openssh().lines().take(3).collect::<Vec<_>>().join("\n") + "\n-----END OPENSSH PRIVATE KEY-----\n";
    assert!(KeyPair::from_openssh(&cut, None).is_err());
}

#[test]
fn ssh_keygen_reads_our_keys_and_we_read_its_keys_including_passphrase_protected_ones() {
    if !have("ssh-keygen") {
        eprintln!("skipped: no ssh-keygen");
        return;
    }
    let d = temp("keygen");
    // ours -> ssh-keygen
    let k = KeyPair::generate("round@trip").unwrap();
    let f = d.join("ours");
    std::fs::write(&f, k.to_openssh().as_bytes()).unwrap();
    std::fs::set_permissions(&f, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let out = Command::new("ssh-keygen").args(["-l", "-f"]).arg(&f).output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{said}{}", String::from_utf8_lossy(&out.stderr));
    assert!(said.contains(&k.fingerprint()) && said.contains("round@trip") && said.contains("ED25519"), "{said}");
    let pubout = Command::new("ssh-keygen").args(["-y", "-f"]).arg(&f).output().unwrap();
    assert!(String::from_utf8_lossy(&pubout.stdout).trim().starts_with(&format!("ssh-ed25519 {}", b64(&k.public_blob()))));
    // theirs, no passphrase
    let g = d.join("theirs");
    assert!(Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", "their@key", "-f"]).arg(&g).status().unwrap().success());
    let theirs = std::fs::read_to_string(&g).unwrap();
    let pubtxt = std::fs::read_to_string(d.join("theirs.pub")).unwrap();
    let kk = KeyPair::from_openssh(&theirs, None).unwrap();
    assert_eq!(kk.public_line(), pubtxt.trim());
    // theirs, with a passphrase (bcrypt + aes256-ctr)
    let h = d.join("locked");
    assert!(Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "correct horse", "-a", "7", "-C", "locked@key", "-f"]).arg(&h).status().unwrap().success());
    let locked = std::fs::read_to_string(&h).unwrap();
    let lpub = std::fs::read_to_string(d.join("locked.pub")).unwrap();
    assert!(is_encrypted(&locked));
    assert_eq!(public_line_of(&locked).unwrap(), lpub.trim().rsplit_once(' ').unwrap().0);
    assert_eq!(KeyPair::from_openssh(&locked, None).unwrap_err(), KeyError::NeedPassphrase);
    assert_eq!(KeyPair::from_openssh(&locked, Some("wrong one")).unwrap_err(), KeyError::WrongPassphrase);
    let ok = KeyPair::from_openssh(&locked, Some("correct horse")).unwrap();
    assert_eq!(ok.public_line(), lpub.trim());
    assert_eq!(ok.comment, "locked@key");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rsa_keys_say_what_to_do_instead() {
    if !have("ssh-keygen") {
        return;
    }
    let d = temp("rsa");
    let f = d.join("rsa");
    assert!(Command::new("ssh-keygen").args(["-q", "-t", "rsa", "-b", "2048", "-N", "", "-f"]).arg(&f).status().unwrap().success());
    let e = KeyPair::from_openssh(&std::fs::read_to_string(&f).unwrap(), None).unwrap_err();
    assert!(matches!(&e, KeyError::Unsupported(s) if s.contains("ssh-rsa") && s.contains("ed25519")), "{e:?}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_short_text_is_short_and_has_no_secret_in_it() {
    let k = KeyPair::generate("a").unwrap();
    let s = k.short_text();
    assert!(s.len() < 40 && s.starts_with("ed25519 "), "{s}");
}
