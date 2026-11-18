//! The QEMU session checks (`dist/qemu/session-checks.py`, docs/development/QEMU-TESTS.md): one boot of an existing image, many named checks.
//!
//! Every test here is `#[ignore]`d: a run needs QEMU, a built image and a built `iyakki` client, and takes minutes. Two ways to run them:
//!
//!   ./run.sh test-qemu                       boots once and runs every check (the runner prints PASS or FAIL per row id and keeps the evidence)
//!   IYAKKI_QEMU_RESULTS=DIR cargo test -p iyakki-core --test qemu_session -- --ignored
//!                                            each test below reads the `results.json` the runner wrote in DIR and asserts its own check passed
//!
//! The tests that read a run do not boot anything themselves, so one boot serves all of them. `run_all_in_qemu` starts the runner itself.
//! The two tests that are not ignored keep the checks list, the runner's functions and these tests in step.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn listed() -> Vec<(String, Vec<String>)> {
    let t = std::fs::read_to_string(root().join("dist/qemu/session-checks.list")).unwrap();
    t.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#') && !l.starts_with("profile:"))
        .map(|l| {
            let mut p = l.splitn(3, '|');
            let name = p.next().unwrap().trim().to_string();
            let rows = p.next().unwrap().split(',').map(|r| r.trim().to_string()).collect();
            (name, rows)
        })
        .collect()
}

/// The status the latest run wrote for one check; the run is the folder named by IYAKKI_QEMU_RESULTS.
fn status_of(name: &str) -> String {
    let dir = std::env::var("IYAKKI_QEMU_RESULTS").expect("set IYAKKI_QEMU_RESULTS to the folder `./run.sh test-qemu` printed as evidence");
    let j = std::fs::read_to_string(Path::new(&dir).join("results.json")).expect("results.json in IYAKKI_QEMU_RESULTS");
    let v: serde_json::Value = serde_json::from_str(&j).unwrap();
    let r = v["results"].as_array().unwrap().iter().find(|r| r["name"] == name).unwrap_or_else(|| panic!("check {name} is not in that run"));
    assert_eq!(r["status"], "PASS", "check {name}: {}", r["evidence"]);
    "PASS".into()
}

#[test]
fn every_listed_check_has_a_function_and_a_test() {
    let defs = ["session_checks_defs.py", "session_checks_ui_defs.py"].iter().map(|f| std::fs::read_to_string(root().join("dist/qemu").join(f)).unwrap()).collect::<Vec<_>>().join("\n");
    let me = std::fs::read_to_string(root().join("crates/iyakki-core/tests/qemu_session.rs")).unwrap();
    let list = listed();
    assert!(!list.is_empty());
    for (name, rows) in &list {
        assert!(defs.contains(&format!("def check_{name}(")), "{name} has no function in session_checks_defs.py");
        assert!(me.contains(&format!("fn {name}()")), "{name} has no test in qemu_session.rs");
        assert!(!rows.is_empty() && rows.iter().all(|r| r.starts_with("F-")), "{name}: rows {rows:?}");
    }
    for l in defs.lines().filter(|l| l.starts_with("def check_")) {
        let n = l.trim_start_matches("def check_").split('(').next().unwrap();
        assert!(list.iter().any(|(x, _)| x == n), "check_{n} is not in session-checks.list");
    }
}

#[test]
fn the_default_layers_do_not_run_the_qemu_session() {
    for f in ["dist/release/test-layer.sh", "dist/scripts/crate-layers.txt"] {
        let t = std::fs::read_to_string(root().join(f)).unwrap_or_default();
        assert!(!t.contains("session-checks") && !t.contains("qemu_session"), "{f} runs the QEMU session");
    }
}

#[test]
#[ignore = "boots QEMU: needs an image and a built client (./run.sh test-qemu)"]
fn run_all_in_qemu() {
    let st = Command::new("python3").arg(root().join("dist/qemu/session-checks.py")).status().unwrap();
    assert!(st.success(), "a check failed or the guest could not be started");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn dev_doctor() {
    status_of("dev_doctor");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn dev_info_compat() {
    status_of("dev_info_compat");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn coldplug_extra_pass() {
    status_of("coldplug_extra_pass");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn components_record() {
    status_of("components_record");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn private_store_roundtrip() {
    status_of("private_store_roundtrip");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn pkg_trust_levels() {
    status_of("pkg_trust_levels");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn device_name() {
    status_of("device_name");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn dev_approval_banner() {
    status_of("dev_approval_banner");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn a11y_labels() {
    status_of("a11y_labels");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn auth_tile() {
    status_of("auth_tile");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn devtools_tile() {
    status_of("devtools_tile");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn home_layout_page() {
    status_of("home_layout_page");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn move_tile_keys() {
    status_of("move_tile_keys");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn desktop_views() {
    status_of("desktop_views");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn theme_switch() {
    status_of("theme_switch");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn power_key_card() {
    status_of("power_key_card");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn editor_run_wasm() {
    status_of("editor_run_wasm");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn doctor_share_preview() {
    status_of("doctor_share_preview");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn flows_templates_and_runs() {
    status_of("flows_templates_and_runs");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn firewall_presets() {
    status_of("firewall_presets");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn router_page() {
    status_of("router_page");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn shared_folders() {
    status_of("shared_folders");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn system_update_page() {
    status_of("system_update_page");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn model_host_scan() {
    status_of("model_host_scan");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn screen_saver_idle() {
    status_of("screen_saver_idle");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn checkup_steps() {
    status_of("checkup_steps");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn store_installed_lifecycle() {
    status_of("store_installed_lifecycle");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn browser_engine_pages() {
    status_of("browser_engine_pages");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn browser_devtools_panel() {
    status_of("browser_devtools_panel");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn browser_short_words() {
    status_of("browser_short_words");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn browser_devtools_honest() {
    status_of("browser_devtools_honest");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn panes_and_chords() {
    status_of("panes_and_chords");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn clock_floor_lifted() {
    status_of("clock_floor_lifted");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn media_samples() {
    status_of("media_samples");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn store_setup_recommended() {
    status_of("store_setup_recommended");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn dev_patch_delta() {
    status_of("dev_patch_delta");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn screen_saver_idle_timer() {
    status_of("screen_saver_idle_timer");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn config_daily_check() {
    status_of("config_daily_check");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn k8s_real_cluster() {
    status_of("k8s_real_cluster");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn system_tile_pods() {
    status_of("system_tile_pods");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn dev_patch_same_id_delta() {
    status_of("dev_patch_same_id_delta");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn quick_install_registry() {
    status_of("quick_install_registry");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn quick_install_gitea() {
    status_of("quick_install_gitea");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn k3s_popup_stop_start() {
    status_of("k3s_popup_stop_start");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn app_events_connectivity() {
    status_of("app_events_connectivity");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn app_events_late_after_away() {
    status_of("app_events_late_after_away");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn app_events_settings_switch() {
    status_of("app_events_settings_switch");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn app_events_flow_and_settings() {
    status_of("app_events_flow_and_settings");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn app_events_waits_while_away() {
    status_of("app_events_waits_while_away");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn app_events_lock() {
    status_of("app_events_lock");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn app_events_permissions() {
    status_of("app_events_permissions");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn app_events_survive_restart() {
    status_of("app_events_survive_restart");
}

#[test]
#[ignore = "reads a QEMU session run (IYAKKI_QEMU_RESULTS)"]
fn browser_data_protection() {
    status_of("browser_data_protection");
}
