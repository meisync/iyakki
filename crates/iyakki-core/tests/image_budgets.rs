//! F-PKG-05: the budget file is well formed and the checker passes, fails and reports as it should (run on made-up image folders).

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-budgets-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(image: &str, dir: &Path, results: Option<&Path>, strict: bool) -> (i32, String) {
    let mut c = Command::new("python3");
    c.arg("-I").arg(root().join("dist/release/check-budgets.py")).args(["--image", image, "--images-dir"]).arg(dir);
    if let Some(r) = results {
        c.arg("--results").arg(r);
    }
    if strict {
        c.arg("--strict");
    }
    let o = c.output().expect("python3 is needed for the budget checker");
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr))
}

fn write_sized(dir: &Path, name: &str, n: usize) {
    std::fs::write(dir.join(name), vec![0u8; n]).unwrap();
}

#[test]
fn every_budget_names_where_its_number_comes_from() {
    let t = std::fs::read_to_string(root().join("dist/release/image-budgets.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&t).unwrap();
    let images = v["images"].as_object().unwrap();
    assert!(images.contains_key("full") && images.contains_key("libre"));
    for (name, img) in images {
        for f in img["files"].as_array().unwrap() {
            assert!(f["id"].is_string() && f["glob"].is_string() && f["source"].as_str().is_some_and(|s| !s.is_empty()), "{name}: file budget without id, glob or source");
            let (mx, got) = (f["max_bytes"].as_u64(), f["measured_bytes"].as_u64());
            if let (Some(mx), Some(got)) = (mx, got) {
                assert!(mx >= got && mx <= got + got / 5, "{name}/{}: the ceiling must be the measurement plus at most 20 percent", f["id"]);
            }
        }
        for m in img["timings"].as_array().unwrap().iter().chain(img["memory"].as_array().unwrap()) {
            assert!(m["id"].is_string() && m["source"].as_str().is_some_and(|s| !s.is_empty()), "{name}: budget without id or source");
        }
    }
}

#[test]
fn an_image_inside_its_budgets_passes_and_missing_pieces_are_reported_not_hidden() {
    let d = tmp("ok");
    write_sized(&d, "bzImage", 13_000_000);
    write_sized(&d, "initrd-lean", 1_200_000);
    let res = d.join("r.json");
    std::fs::write(&res, r#"{"timings": {"login_wall_s": 8.5}}"#).unwrap();
    let (code, out) = run("full", &d, Some(&res), false);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("ok         kernel") && out.contains("ok         login_wall_s"), "{out}");
    assert!(out.contains("unmeasured") && out.contains("initrd-full") && out.contains("no budget") && out.contains("idle_ram_mib"), "{out}");
    let (strict, _) = run("full", &d, Some(&res), true);
    assert_eq!(strict, 1, "--strict fails on a budget nobody measured");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn growth_past_a_ceiling_and_a_slow_boot_fail_with_the_amount() {
    let d = tmp("over");
    write_sized(&d, "bzImage", 16_000_000);
    write_sized(&d, "x.iso", 71_000_000);
    let res = d.join("r.json");
    std::fs::write(&res, r#"{"timings": {"login_wall_s": 15.0}}"#).unwrap();
    let (code, out) = run("full", &d, Some(&res), false);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("over") && out.contains("16000000 bytes is over the 15000000 budget by 1000000 bytes"), "{out}");
    assert!(out.contains("15.0 is over the 12.0 budget"), "{out}");
    let (code, out) = run("libre", &d, None, false);
    assert_eq!(code, 1, "{out}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_wrong_image_name_or_budget_file_is_a_usage_error() {
    let d = tmp("bad");
    let (code, out) = run("nope", &d, None, false);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("unknown image"), "{out}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_measure_script_checks_a_real_folder_suggests_ceilings_and_says_when_it_cannot_run() {
    let d = tmp("measure");
    write_sized(&d, "bzImage", 13_000_000);
    let out = Command::new("sh").arg(root().join("dist/release/measure-budgets.sh")).arg(&d).args(["full", "0"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("ok         kernel") && text.contains("suggested") && text.contains("\"max_bytes\": 14300000"), "{text}");
    // a size over the ceiling fails the script
    write_sized(&d, "bzImage", 16_000_000);
    let out = Command::new("sh").arg(root().join("dist/release/measure-budgets.sh")).arg(&d).args(["full", "0"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    // no folder: a usage error, not a pass
    let out = Command::new("sh").arg(root().join("dist/release/measure-budgets.sh")).arg(d.join("missing")).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(&d);
}

fn gate(args: &[&Path]) -> (i32, String) {
    let o = Command::new("sh").arg(root().join("dist/release/budgets-gate.sh")).args(args).output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr))
}

#[test]
fn the_budgets_gate_passes_a_small_image_fails_an_oversize_one_and_refuses_a_missing_folder() {
    let ok = tmp("gate-ok");
    write_sized(&ok, "bzImage", 1000);
    let (code, out) = gate(&[&ok]);
    assert_eq!(code, 0, "{out}");
    assert!(out.lines().last().unwrap().starts_with("budgets-gate: PASS"), "{out}");
    let big = tmp("gate-big");
    write_sized(&big, "bzImage", 16_000_000);
    let (code, out) = gate(&[&ok, &big]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("over") && out.lines().last().unwrap().starts_with("budgets-gate: FAIL"), "{out}");
    let (code, out) = gate(&[&ok.join("missing")]);
    assert_eq!(code, 2, "{out}");
}

#[test]
fn alpha_check_runs_the_budgets_gate_only_when_asked() {
    let t = std::fs::read_to_string(root().join("dist/release/alpha-check.sh")).unwrap();
    assert!(t.contains("--budgets) BUDGETS=1") && t.contains("dist/release/budgets-gate.sh"), "the flag and the call are in the script");
    assert!(t.contains("not asked for (--budgets)"), "without the flag the row says it was skipped");
}


#[test]
fn the_boot_measure_reads_the_ram_in_use_at_the_sign_in_screen_from_the_serial_log() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmp("ram");
    std::fs::create_dir_all(d.join("images")).unwrap();
    write_sized(&d.join("images"), "bzImage", 10);
    write_sized(&d.join("images"), "rootfs.cpio.zst", 10);
    // a stand-in for qemu: prints what the real serial console prints, with the memory line right after the first picture
    let fake = d.join("fake-qemu.sh");
    std::fs::write(&fake, "#!/bin/sh\necho '[    0.000000] Linux version 6.12'\necho '[    1.0] Run /init as init process'\necho '[    2.0] supervisor up'\necho '[    4.0] shell: first picture on screen (1280x800, first frame took 5 ms, 20 ms after the display was opened)'\necho '[    4.0] shell: memory in use at the sign-in screen: 412 MiB of 1950 MiB'\nsleep 30\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let json = d.join("res.json");
    let o = Command::new("python3").arg("-I").arg(root().join("dist/buildroot/boot-time.py")).arg(&d).arg("1").env("QEMU", &fake).env("BOOT_TIME_JSON", &json).output().unwrap();
    let text = String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success() && text.contains("RAM in use at the sign-in screen 412 MiB"), "{text}");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(v["memory"]["idle_ram_mib"], 412, "{v}");
    assert!(v["timings"]["login_wall_s"].is_number());
    // the checker takes the figure: no ceiling is written yet, so it is reported as a figure without a budget and never fails
    let (code, out) = run("full", &d.join("images"), Some(&json), false);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("idle_ram_mib") && out.contains("412"), "{out}");
    let _ = std::fs::remove_dir_all(&d);
}
