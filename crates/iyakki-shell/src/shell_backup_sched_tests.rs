//! The scheduled backup in the shell: a due run starts by itself while a person is signed in, never while the screen is locked or twice at once,
//! writes one file, records how it went and waits for its next slot. The clock is a field, not the wall clock; the worker thread is joined by polling.

use super::tests::*;
use super::*;
use iyakki_agent::backup::Layout;
use iyakki_agent::backup_schedule::Schedule;

const NOW: i64 = 1_794_312_000;

fn rig(tag: &str) -> (Shell, std::path::PathBuf, std::path::PathBuf) {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "alice", "alicepass1");
    let root = std::env::temp_dir().join(format!("iyakki-shbk-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (data, home, stick) = (root.join("data"), root.join("home"), root.join("stick"));
    for d in [&data, &home, &stick] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(data.join("layout-alice.json"), "{\"tiles\":1}").unwrap();
    sh.bksched.layout = Some(Layout { data, home, user: "alice".into() });
    sh.bksched.now = Some(NOW);
    (sh, root, stick)
}

fn beat(sh: &mut Shell) {
    sh.tick += 25;
    sh.backup_schedule_poll();
}

/// Poll until the worker has been taken (the thread is a few milliseconds of work).
fn finish(sh: &mut Shell) {
    for _ in 0..400 {
        beat(sh);
        if !sh.backup_schedule_running_for_test() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the scheduled backup did not finish");
}

fn files(stick: &std::path::Path) -> usize {
    std::fs::read_dir(stick).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "iybak")).count()
}

#[test]
fn a_due_schedule_runs_once_by_itself_records_the_result_and_waits_for_its_next_slot() {
    let (mut sh, _root, stick) = rig("due");
    let dir = sh.env.services.dir().to_path_buf();
    // nothing scheduled: nothing happens
    beat(&mut sh);
    assert!(!sh.backup_schedule_running_for_test() && files(&stick) == 0);
    let s = Schedule { enabled: true, every_days: 1, hour: 0, dest: stick.display().to_string(), keep: 3, ..Schedule::default() };
    s.save(&dir, "alice").unwrap();
    beat(&mut sh);
    assert!(sh.backup_schedule_running_for_test(), "due at once: it never ran and the hour has passed");
    beat(&mut sh);
    finish(&mut sh);
    assert_eq!(files(&stick), 1);
    let after = Schedule::load(&dir, "alice");
    assert_eq!(after.last_run, NOW);
    assert!(after.last_result.starts_with("done, ") && after.enabled, "{}", after.last_result);
    // the next look finds it not due: no second file
    for _ in 0..5 {
        beat(&mut sh);
    }
    assert!(!sh.backup_schedule_running_for_test() && files(&stick) == 1);
    // a day later it is due again
    sh.bksched.now = Some(NOW + 2 * 86_400);
    beat(&mut sh);
    finish(&mut sh);
    assert_eq!(files(&stick), 2, "a second one a day later (the stamp of the name is the clock's)");
    let audit = std::fs::read_to_string(sh.env.audit.path()).unwrap_or_default();
    assert!(audit.contains("backup.scheduled"), "{audit}");
}

#[test]
fn a_locked_screen_a_missing_folder_and_a_signed_out_person_start_nothing_and_a_failure_is_not_retried() {
    let (mut sh, root, stick) = rig("blocked");
    let dir = sh.env.services.dir().to_path_buf();
    let s = Schedule { enabled: true, every_days: 1, hour: 0, dest: stick.display().to_string(), keep: 3, ..Schedule::default() };
    s.save(&dir, "alice").unwrap();
    assert!(sh.lock_screen("test"));
    beat(&mut sh);
    assert!(!sh.backup_schedule_running_for_test() && files(&stick) == 0, "locked: the person is not there");
    sh.sign_out();
    beat(&mut sh);
    assert!(!sh.backup_schedule_running_for_test(), "nobody is signed in");
    login(&mut sh, "alice", "alicepass1");
    let gone = root.join("unplugged");
    Schedule { dest: gone.display().to_string(), ..s }.save(&dir, "alice").unwrap();
    beat(&mut sh);
    finish(&mut sh);
    let after = Schedule::load(&dir, "alice");
    assert!(after.last_result.starts_with("failed: ") && after.last_result.contains("is not there"), "{}", after.last_result);
    for _ in 0..5 {
        beat(&mut sh);
    }
    assert!(!sh.backup_schedule_running_for_test(), "a failure waits for the next slot");
}
