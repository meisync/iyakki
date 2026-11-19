//! The opt-in QEMU proofs (`dist/qemu/`, F-LINAPP-13, F-BOOT-30, F-BOOT-31, F-INST-13): what a Mac can check about them. The guest runs themselves need
//! a Linux PC with KVM; here: the guest's result lines and the script that reads them agree, the scripts parse, refuse to run without images, are
//! described in the README, and are not part of any default gate.

use std::path::{Path, PathBuf};
use std::process::Command;

use iyakki_core::selfcheck::{check, lines};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const SCRIPTS: [&str; 4] = ["sandbox-net-test.sh", "boot-prefetch-test.sh", "compressed-memory-test.sh", "bios-shared-disk-test.sh"];

fn read_log(tag: &str, mode: &str, text: &str) -> (i32, String) {
    let d = std::env::temp_dir().join(format!("iyakki-qemu-scripts-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("serial.log");
    std::fs::write(&f, text).unwrap();
    let o = Command::new("python3").arg("-I").arg(root().join("dist/qemu/selftest-result.py")).arg(&f).arg(mode).output().expect("python3");
    let _ = std::fs::remove_dir_all(&d);
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).to_string())
}

fn serial(guest: &[String]) -> String {
    // what a serial console really gives: kernel noise, carriage returns, time stamps in front, the login prompt after
    let mut s = String::from("[    0.000000] Linux version 6.12\r\n[    2.1] IYAKKI-BOOT zram: zram0\r\n");
    for l in guest {
        s.push_str(&format!("[   10.5] {l}\r\n"));
    }
    s.push_str("IYAKKI login: ");
    s
}

#[test]
fn the_guests_result_lines_are_read_the_same_way_by_the_script() {
    let good = vec![check("table-loaded", true, "iyakki_sbx_0007"), check("allowed-reachable", true, "10.0.2.2:18080 -> Some(true)"), check("teardown-veth", true, "no ivh interface left")];
    let (code, out) = read_log("good", "sandboxnet", &serial(&lines("sandboxnet", &good)));
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("PASS  table-loaded") && out.trim_end().ends_with("PASS"), "{out}");
    // a failed check fails the run and is named
    let mut bad = good.clone();
    bad[1] = check("allowed-reachable", false, "10.0.2.2:18080 -> Some(false)");
    let (code, out) = read_log("bad", "sandboxnet", &serial(&lines("sandboxnet", &bad)));
    assert_eq!(code, 1);
    assert!(out.contains("FAIL  allowed-reachable") && out.contains("failed: allowed-reachable"), "{out}");
    // a guest that did not finish (timeout, panic): checks but no RESULT
    let all = lines("sandboxnet", &good);
    let (code, out) = read_log("cut", "sandboxnet", &serial(&all[..2]));
    assert_eq!(code, 1);
    assert!(out.contains("no RESULT"), "{out}");
    // nothing at all, and another mode's lines, are not a pass
    assert_eq!(read_log("none", "sandboxnet", "booted but no self-test ran\n").0, 1);
    assert_eq!(read_log("mode", "bootlist", &serial(&all)).0, 1);
    // a RESULT that lies about its checks
    let liar = "IYAKKI-SELFTEST sandboxnet check a PASS x\nIYAKKI-SELFTEST sandboxnet RESULT PASS 5/5\n";
    let (code, out) = read_log("liar", "sandboxnet", liar);
    assert_eq!(code, 1);
    assert!(out.contains("does not match"), "{out}");
    // the Rust reader and the script agree on every one of these logs
    for log in [serial(&all), serial(&lines("sandboxnet", &bad)), serial(&all[..2]), liar.to_string()] {
        let p = iyakki_core::selfcheck::parse(&log, "sandboxnet");
        let want = iyakki_core::selfcheck::verdict(&p).is_ok();
        assert_eq!(read_log("agree", "sandboxnet", &log).0 == 0, want, "{log}");
    }
    assert_eq!(read_log("usage", "x", "").0, 1);
    let o = Command::new("python3").arg("-I").arg(root().join("dist/qemu/selftest-result.py")).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "no arguments is a usage error");
}

#[test]
fn every_script_parses_says_what_it_needs_and_refuses_to_pretend_without_images() {
    let readme = std::fs::read_to_string(root().join("dist/qemu/README.md")).unwrap();
    for s in SCRIPTS {
        let path = root().join("dist/qemu").join(s);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert!(text.starts_with("#!/bin/sh"), "{s}");
        assert!(text.contains("Needs:") && text.contains("PASS"), "{s} must say what it needs and what PASS is");
        assert!(readme.contains(s), "the README does not describe {s}");
        let o = Command::new("sh").arg("-n").arg(&path).output().unwrap();
        assert!(o.status.success(), "{s}: {}", String::from_utf8_lossy(&o.stderr));
        // a folder without images is "nothing was tested" (exit 2), never a pass
        let empty = std::env::temp_dir().join(format!("iyakki-qemu-noimg-{}-{s}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let o = Command::new("sh").arg(&path).arg(&empty).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{s}: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        assert!(String::from_utf8_lossy(&o.stderr).contains("no images"), "{s}");
        let _ = std::fs::remove_dir_all(&empty);
    }
    for words in ["KVM", "PASS means", "selftest-result.py", "alpha-check"] {
        assert!(readme.contains(words), "README lacks `{words}`");
    }
}

#[test]
fn nothing_in_the_default_gates_runs_these_scripts() {
    for gate in ["run.sh", "dist/release/alpha-check.sh", "dist/release/release.sh", "dist/release/run-window.sh"] {
        let t = std::fs::read_to_string(root().join(gate)).unwrap_or_default();
        assert!(!t.contains("dist/qemu"), "{gate} runs an opt-in QEMU proof");
    }
}

#[test]
fn the_guest_modes_exist_in_the_program_and_the_scripts_ask_for_them() {
    let main = std::fs::read_to_string(root().join("crates/iyakki-core/src/main.rs")).unwrap();
    for (mode, script) in [("sandboxnet", "sandbox-net-test.sh"), ("bootlist", "boot-prefetch-test.sh")] {
        assert!(main.contains(&format!("Some(\"{mode}\")")), "{mode} is not a self-test mode");
        let s = std::fs::read_to_string(root().join("dist/qemu").join(script)).unwrap();
        assert!(s.contains(&format!("iyakki.selftest={mode}")) && s.contains(&format!("result \"$WORK/")), "{script} does not run {mode}");
    }
    for want in ["zswap enabled", "swapfile "] {
        assert!(main.contains(want), "mem_report lacks `{want}`");
    }
}
