use super::*;
use serde_json::Value;

struct Folder(PathBuf);
impl browserpkg::Source for Folder {
    fn manifest(&self) -> Result<Value, String> {
        serde_json::from_slice(&std::fs::read(self.0.join("manifest.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
    fn download(&self, dest: &Path) -> Result<(), String> {
        std::fs::copy(self.0.join("webview-helper"), dest).map(|_| ()).map_err(|e| e.to_string())
    }
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-pkgbuild-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn key(n: u8) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[n; 32])
}

fn helper(d: &Path, body: &[u8]) -> PathBuf {
    let p = d.join("webview-helper-in");
    let mut bytes = vec![0x7f, b'E', b'L', b'F'];
    bytes.extend_from_slice(body);
    std::fs::write(&p, bytes).unwrap();
    p
}

#[test]
fn a_built_package_is_read_by_the_card_and_the_install_and_the_program_arrives_byte_for_byte() {
    let d = tmp("roundtrip");
    let h = helper(&d, &[9u8; 3000]);
    let out = build(&h, "2026.11-1", "x86_64", &key(5), &d.join("store")).unwrap();
    assert!(out.ends_with("store/browser/x86_64") && out.join("webview-helper").is_file() && out.join("manifest.json").is_file());
    assert!(!out.join("webview-helper.part").exists() && !out.join("manifest.json.part").exists());
    let pin = crate::pkg::public_key_hex(&key(5));
    let src = Folder(out.clone());
    let data = d.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let card = browserpkg::card(&data, &src, &pin, "x86_64").unwrap();
    assert_eq!((card.version.as_str(), card.size), ("2026.11-1", 3004));
    assert_eq!(browserpkg::install(&data, &src, &pin, "x86_64", true).unwrap(), "2026.11-1");
    assert_eq!(std::fs::read(browserpkg::installed_helper(&data).unwrap()).unwrap(), std::fs::read(&h).unwrap());
    // another key is not the pinned one: nothing installs
    let other = browserpkg::install(&d.join("data2"), &src, &crate::pkg::public_key_hex(&key(6)), "x86_64", true);
    assert!(other.unwrap_err().contains("not signed by the store key"));
    // a changed byte after building is caught by the signed checksum
    std::fs::write(out.join("webview-helper"), [vec![0x7f, b'E', b'L', b'F'], vec![8u8; 3000]].concat()).unwrap();
    assert!(browserpkg::install(&d.join("data3"), &src, &pin, "x86_64", true).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_helper_that_is_not_a_program_is_empty_or_missing_and_odd_names_are_refused_and_nothing_is_left() {
    let d = tmp("refuse");
    let plain = d.join("plain");
    std::fs::write(&plain, b"#!/bin/sh\necho hi\n").unwrap();
    assert!(build(&plain, "1", "x86_64", &key(1), &d.join("o")).unwrap_err().contains("not a program"));
    let empty = d.join("empty");
    std::fs::write(&empty, b"").unwrap();
    assert!(build(&empty, "1", "x86_64", &key(1), &d.join("o")).is_err());
    assert!(build(&d.join("missing"), "1", "x86_64", &key(1), &d.join("o")).is_err());
    let h = helper(&d, b"x");
    for bad in ["", "1 2", "../1", "v/1", &"9".repeat(41)] {
        assert!(build(&h, bad, "x86_64", &key(1), &d.join("o")).unwrap_err().contains("version"), "{bad:?}");
    }
    assert!(build(&h, "1", "x86/64", &key(1), &d.join("o")).unwrap_err().contains("architecture"));
    assert!(!d.join("o").exists(), "nothing was written for a refusal");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_command_builds_with_a_key_file_prints_only_the_public_key_and_refuses_a_bad_key_file() {
    let d = tmp("cli");
    let h = helper(&d, &[1u8; 100]);
    let keyfile = d.join("key.hex");
    std::fs::write(&keyfile, format!("{}\n", crate::pkg::signing_key_hex(&key(3)))).unwrap();
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let out = d.join("out");
    let args = s(&["build", "--helper", h.to_str().unwrap(), "--version", "7", "--key-file", keyfile.to_str().unwrap(), "--out", out.to_str().unwrap(), "--arch", "aarch64"]);
    assert_eq!(cli_main(&args), 0);
    let doc: Value = serde_json::from_slice(&std::fs::read(out.join("browser/aarch64/manifest.json")).unwrap()).unwrap();
    assert_eq!(doc["key"], crate::pkg::public_key_hex(&key(3)));
    assert!(!std::fs::read_to_string(out.join("browser/aarch64/manifest.json")).unwrap().contains(&crate::pkg::signing_key_hex(&key(3))), "the private key is not in the description");
    assert_eq!(cli_main(&s(&["pubkey", "--key-file", keyfile.to_str().unwrap()])), 0);
    // not a key file, a missing flag, an unknown word
    let bad = d.join("bad");
    std::fs::write(&bad, "hello").unwrap();
    assert_eq!(cli_main(&s(&["pubkey", "--key-file", bad.to_str().unwrap()])), 1);
    assert_eq!(cli_main(&s(&["build", "--helper", h.to_str().unwrap()])), 2);
    assert_eq!(cli_main(&s(&["frob"])), 2);
    assert_eq!(cli_main(&[]), 2);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_image_option_moves_the_helper_out_of_the_image_and_the_script_hands_it_to_the_builder() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mk = std::fs::read_to_string(root.join("dist/buildroot/package/iyakki-webview-helper/iyakki-webview-helper.mk")).unwrap();
    let cfg = std::fs::read_to_string(root.join("dist/buildroot/package/iyakki-webview-helper/Config.in")).unwrap();
    let sh = std::fs::read_to_string(root.join("dist/webview/make-browser-package.sh")).unwrap();
    assert!(cfg.contains("config BR2_PACKAGE_IYAKKI_WEBVIEW_HELPER_PACKAGE_ONLY") && cfg.contains("depends on BR2_PACKAGE_IYAKKI_WEBVIEW_HELPER"));
    // with the option the program is not installed into the target; without it, it is, as before
    let at = mk.find("ifeq ($(BR2_PACKAGE_IYAKKI_WEBVIEW_HELPER_PACKAGE_ONLY),y)").expect("the switch");
    let (on, off) = mk[at..].split_once("\nelse\n").unwrap();
    assert!(on.contains("$(BINARIES_DIR)/browser-package/webview-helper") && !on.contains("$(TARGET_DIR)/usr/libexec/iyakki/webview-helper"));
    assert!(off.contains("$(TARGET_DIR)/usr/libexec/iyakki/webview-helper") && !off.contains("BINARIES_DIR"));
    // the script passes the key by path, never reads it, and calls the builder of this crate
    assert!(sh.contains("browser-package build --helper") && sh.contains("--key-file \"$KEY\"") && !sh.contains("cat \"$KEY\"") && sh.contains("BR2_PACKAGE_IYAKKI_WEBVIEW_HELPER_PACKAGE_ONLY=y"));
}
