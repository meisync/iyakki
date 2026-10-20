//! Secure Boot (shim + MOK) build scripts and settings: what can be checked without hardware or QEMU. The boot chain itself is
//! tested by `dist/buildroot/qemu-secureboot-test.py` (docs/SECURE-BOOT.md). No real key is in the repository: the tests make
//! throwaway ones in a temp directory.
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}
fn have(tool: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {tool}")]).output().is_ok_and(|o| o.status.success())
}

#[test]
fn the_scripts_parse_and_the_image_hooks_call_the_signing_step_only_with_a_key() {
    for s in ["dist/buildroot/secureboot.sh", "dist/buildroot/secureboot-testkit.sh", "dist/buildroot/board/iyakki/post-image.sh", "dist/buildroot/board/iyakki/post-build.sh"] {
        let st = Command::new("sh").arg("-n").arg(root().join(s)).status().unwrap();
        assert!(st.success(), "{s} does not parse");
    }
    let image = read("dist/buildroot/board/iyakki/post-image.sh");
    assert!(image.contains("IYAKKI_SB_KEY") && image.contains("secureboot.sh\" image") && image.contains("UNSIGNED image"), "post-image.sh: signed branch, and the unsigned build says so");
    let build = read("dist/buildroot/board/iyakki/post-build.sh");
    assert!(build.contains("sign-modules") && build.contains("IYAKKI_SB_CERT"));
    py_parses("dist/buildroot/qemu-secureboot-test.py");
}

fn py_parses(rel: &str) {
    if have("python3") {
        let st = Command::new("python3").args(["-I", "-c", "import ast,sys; ast.parse(open(sys.argv[1]).read())"]).arg(root().join(rel)).status().unwrap();
        assert!(st.success(), "{rel} does not parse");
    }
}

#[test]
fn the_download_is_pinned_by_sha256_and_comes_from_a_distribution() {
    let sb = read("dist/buildroot/secureboot.sh");
    let pins: Vec<&str> = sb.lines().filter(|l| l.contains("_SHA256=\"")).collect();
    assert!(pins.len() >= 4, "{pins:?}");
    for l in pins {
        let v = l.split('"').nth(1).unwrap();
        assert!(v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()), "not a SHA-256: {l}");
    }
    assert!(sb.contains("deb.debian.org") && sb.contains("Microsoft Corporation UEFI CA"), "the download is checked for Microsoft's signature");
    // no firmware boot entries: those files are never put on the disk
    assert!(sb.contains("fbx64.efi must not be on the disk"));
}

#[test]
fn keys_are_made_on_demand_never_overwritten_and_never_stored_in_the_repository() {
    if !have("openssl") {
        eprintln!("openssl missing: skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-sbkey-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let sh = root().join("dist/buildroot/secureboot.sh");
    let run = |args: &[&str]| Command::new("sh").arg(&sh).args(args).output().unwrap();
    assert!(run(&["genkey", dir.to_str().unwrap(), "Test key"]).status.success());
    for f in ["iyakki-sb.key", "iyakki-sb.crt", "iyakki-sb.der"] {
        assert!(dir.join(f).exists(), "{f}");
    }
    let again = run(&["genkey", dir.to_str().unwrap()]);
    assert!(!again.status.success() && String::from_utf8_lossy(&again.stderr).contains("refusing"), "an existing key is never replaced");
    let text = Command::new("openssl").args(["x509", "-noout", "-text", "-in"]).arg(dir.join("iyakki-sb.crt")).output().unwrap();
    let text = String::from_utf8_lossy(&text.stdout).to_string();
    assert!(text.contains("Code Signing") && text.contains("CA:FALSE") && text.contains("Digital Signature"), "{text}");
    assert!(!text.contains("2312.16.1.2"), "the module-signing OID makes shim refuse the key for boot programs");
    // without a key the check says what is missing and fails
    let chk = Command::new("sh").arg(&sh).arg("check").env_remove("IYAKKI_SB_KEY").env_remove("IYAKKI_SB_CERT").output().unwrap();
    assert!(!chk.status.success() && String::from_utf8_lossy(&chk.stdout).contains("IYAKKI_SB_KEY"));
    let _ = std::fs::remove_dir_all(&dir);
    // nothing secret in the tree
    for rel in ["dist", "docs"] {
        fn walk(d: &Path, hits: &mut Vec<String>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, hits);
                } else if std::fs::read(&p).is_ok_and(|b| b.windows(27).any(|w| w == b"-----BEGIN PRIVATE KEY-----")) {
                    hits.push(p.display().to_string());
                }
            }
        }
        let mut hits = vec![];
        walk(&root().join(rel), &mut hits);
        assert!(hits.is_empty(), "private key material in the tree: {hits:?}");
    }
}

#[test]
fn the_kernel_is_set_up_for_signed_modules_and_lockdown_on_every_x86_image() {
    let frag = read("dist/buildroot/board/iyakki/linux.fragment");
    for want in ["CONFIG_MODULE_SIG=y", "# CONFIG_MODULE_SIG_FORCE is not set", "# CONFIG_MODULE_SIG_ALL is not set", "CONFIG_SECURITY_LOCKDOWN_LSM=y", "CONFIG_INTEGRITY_MACHINE_KEYRING=y", "CONFIG_LOAD_UEFI_KEYS=y", "CONFIG_SECONDARY_TRUSTED_KEYRING=y", "CONFIG_SYSTEM_BLACKLIST_KEYRING=y"] {
        assert!(frag.lines().any(|l| l == want), "linux.fragment lacks {want}");
    }
    // MODULE_SIG_FORCE stays off so driver packs load with Secure Boot off; lockdown refuses unsigned modules when it is on
    assert!(!frag.lines().any(|l| l == "CONFIG_MODULE_SIG_FORCE=y"));
    let def = read("dist/buildroot/configs/iyakki_x86_64_defconfig");
    assert!(def.contains("board/iyakki/patches-sb"));
    let patch = std::fs::read_dir(root().join("dist/buildroot/board/iyakki/patches-sb/linux/6.12.112")).unwrap().count();
    assert_eq!(patch, 1, "exactly the one lockdown patch");
    let test_frag = read("dist/buildroot/board/iyakki/secureboot-test.fragment");
    assert!(test_frag.contains("CONFIG_INTEGRITY_MACHINE_KEYRING=y") && test_frag.contains("CONFIG_EFI_STUB=y"));
}

#[test]
fn the_docs_say_what_secure_boot_needs_and_the_known_limits() {
    let d = read("docs/SECURE-BOOT.md");
    for w in ["IYAKKI_SB_KEY", "Enroll key from disk", "iyakki-mok.cer", "initial RAM disk is not signed", "T2", "Startup Security Utility", "SBAT", "driver pack", "Verified in QEMU", "Only on real hardware"] {
        assert!(d.contains(w), "docs/SECURE-BOOT.md lacks '{w}'");
    }
    let lic = read("LICENSING.md");
    assert!(lic.contains("shim") && lic.contains("BSD-2-Clause") && lic.contains("sbsigntool"));
    assert!(read("KNOWN-ISSUES.md").contains("SECURE-BOOT.md"));
}
