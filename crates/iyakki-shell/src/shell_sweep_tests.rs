//! F-LIFE-07: the start-up sweep of temp files a crash left beside saved data, and its audit line.

use super::sweep_leftover_temp;
use crate::accounts::AuditLog;

fn root(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-shell-sweep-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("data/runs/r1")).unwrap();
    d
}

#[test]
fn the_startup_sweep_removes_the_files_of_dead_processes_keeps_the_rest_and_audits_the_count() {
    let d = root("on");
    let data = d.join("data");
    for f in ["settings.json", "settings.json.tmp4242-0", "settings.json.tmp77-3", "runs/r1/run.json.tmp4242-9", "runs/r1/run.json"] {
        std::fs::write(data.join(f), "x").unwrap();
    }
    let audit = AuditLog::new(&d.join("audit.log"));
    let n = sweep_leftover_temp(&data, &audit, &|pid| pid == 77);
    assert_eq!(n, 2);
    assert!(!data.join("settings.json.tmp4242-0").exists() && !data.join("runs/r1/run.json.tmp4242-9").exists());
    assert!(data.join("settings.json").exists() && data.join("settings.json.tmp77-3").exists() && data.join("runs/r1/run.json").exists(), "a live process's temp file and the saved data stay");
    let lines = audit.recent(5);
    assert_eq!(lines.iter().filter(|l| l.contains("data.tmp.swept")).count(), 1, "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("2 leftover temp file(s) removed")));
    assert!(audit.verify().is_ok());
    // a second start finds nothing and writes no line
    assert_eq!(sweep_leftover_temp(&data, &audit, &|pid| pid == 77), 0);
    assert_eq!(audit.recent(5).iter().filter(|l| l.contains("data.tmp.swept")).count(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_clean_data_folder_writes_no_audit_line_and_a_missing_one_is_no_error() {
    let d = root("clean");
    let audit = AuditLog::new(&d.join("audit.log"));
    assert_eq!(sweep_leftover_temp(&d.join("data"), &audit, &|_| false), 0);
    assert_eq!(sweep_leftover_temp(&d.join("not-there"), &audit, &|_| false), 0);
    assert!(audit.recent(5).is_empty());
    let _ = std::fs::remove_dir_all(&d);
}
