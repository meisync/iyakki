//! Scheduled backups: when a run is due (computed in a given zone, never from the wall clock), the settings checks, a run that writes and prunes,
//! the passphrase read from a file, and what a hand-edited file becomes. No test waits on a clock.

use super::*;
use iyakki_clock::Zone;
use std::fs;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-bsched-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn layout(root: &Path, user: &str) -> Layout {
    let l = Layout { data: root.join("data"), home: root.join("home").join(user), user: user.into() };
    fs::create_dir_all(&l.data).unwrap();
    fs::create_dir_all(&l.home).unwrap();
    fs::write(l.data.join(format!("layout-{user}.json")), "{\"tiles\":1}").unwrap();
    l
}

fn sched(dest: &Path) -> Schedule {
    Schedule { enabled: true, every_days: 2, hour: 3, dest: dest.display().to_string(), keep: 2, ..Schedule::default() }
}

const DAY: i64 = 86_400;
/// 2026-11-10 12:00:00 UTC
const NOON: i64 = 1_794_312_000;

#[test]
fn a_run_is_due_at_the_hour_n_days_after_the_last_one_and_at_once_when_it_never_ran() {
    let z = Zone::utc();
    let mut s = sched(Path::new("/media/stick"));
    // never ran: today's hour; it is past 03:00 at noon, so it is due now
    assert!(s.due(&z, NOON));
    s.last_run = NOON;
    let slot = s.next_slot(&z, NOON);
    assert_eq!(slot, NOON - 12 * 3600 + 2 * DAY + 3 * 3600, "03:00 two days after the day of the last run");
    assert!(!s.due(&z, slot - 1) && s.due(&z, slot) && s.due(&z, slot + 5 * DAY), "a computer that was off runs it when it is back");
    s.enabled = false;
    assert!(!s.due(&z, slot + DAY), "off is off");
    s.enabled = true;
    s.dest = String::new();
    assert!(!s.due(&z, slot + DAY), "no folder, no run");
}

#[test]
fn the_settings_are_checked_and_a_hand_edited_file_is_brought_back_inside_the_limits() {
    let d = tmp("valid");
    let s = sched(&d);
    assert!(s.validate().is_ok());
    for bad in [Schedule { every_days: 0, ..s.clone() }, Schedule { every_days: 32, ..s.clone() }, Schedule { hour: 24, ..s.clone() }, Schedule { keep: 0, ..s.clone() }, Schedule { keep: 51, ..s.clone() }, Schedule { dest: "relative/dir".into(), ..s.clone() }, Schedule { dest: "/a/../b".into(), ..s.clone() }, Schedule { passphrase_file: "pass.txt".into(), ..s.clone() }, Schedule { areas: vec!["nonsense".into()], ..s.clone() }] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
    s.save(&d, "ann").unwrap();
    assert_eq!(Schedule::load(&d, "ann"), s);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(path(&d, "ann")).unwrap().permissions().mode() & 0o777, 0o600);
    }
    fs::write(path(&d, "bo"), r#"{"enabled":true,"every_days":900,"hour":99,"keep":0,"areas":["settings","x"]}"#).unwrap();
    let r = Schedule::load(&d, "bo");
    assert_eq!((r.every_days, r.hour, r.keep, r.areas.len()), (31, 23, 1, 1));
    assert_eq!(Schedule::load(&d, "nobody"), Schedule::default(), "no file: off");
}

#[test]
fn a_run_writes_one_file_prunes_the_oldest_of_this_account_only_and_records_the_result() {
    let root = tmp("run");
    let l = layout(&root, "amy");
    let dest = root.join("stick");
    fs::create_dir_all(&dest).unwrap();
    // another account's file and a stranger's file are never touched
    fs::write(dest.join("iyakki-backup-bob-20260101-000000.iybak"), "x").unwrap();
    fs::write(dest.join("notes.txt"), "x").unwrap();
    let mut s = sched(&dest);
    let mut files = Vec::new();
    for i in 0..3 {
        let c = run(&l, &mut s, 1_800_000_000 + i * DAY, &Progress::default()).unwrap();
        assert!(!c.encrypted);
        files.push(c.path);
    }
    let left: Vec<String> = fs::read_dir(&dest).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(left.iter().filter(|n| n.starts_with("iyakki-backup-amy-")).count(), 2, "keep 2: {left:?}");
    assert!(!files[0].exists() && files[1].exists() && files[2].exists(), "the oldest went");
    assert!(left.contains(&"iyakki-backup-bob-20260101-000000.iybak".to_string()) && left.contains(&"notes.txt".to_string()));
    assert_eq!(s.last_run, 1_800_000_000 + 2 * DAY);
    assert!(s.last_result.starts_with("done, ") && s.last_result.contains("not encrypted") && s.last_result.contains("removed 1"), "{}", s.last_result);
}

#[test]
fn an_encrypted_schedule_reads_the_passphrase_file_and_failures_are_said_and_still_move_the_slot() {
    let root = tmp("enc");
    let l = layout(&root, "amy");
    let dest = root.join("stick");
    fs::create_dir_all(&dest).unwrap();
    let pf = root.join("pass.txt");
    fs::write(&pf, "correct horse battery\nsecond line ignored\n").unwrap();
    let mut s = Schedule { passphrase_file: pf.display().to_string(), ..sched(&dest) };
    let c = run(&l, &mut s, 1_800_000_000, &Progress::default()).unwrap();
    assert!(c.encrypted && s.last_result.contains("encrypted") && !s.last_result.contains("not encrypted"));
    assert!(!fs::read(&c.path).unwrap().windows(21).any(|w| w == b"correct horse battery"), "the passphrase is not in the file");
    assert!(!s.describe(&Zone::utc(), 1_800_000_100).contains("correct horse"), "the status never shows it");
    // a short passphrase, a missing file, a missing folder
    fs::write(&pf, "short\n").unwrap();
    assert!(run(&l, &mut s, 1_800_000_000 + DAY, &Progress::default()).unwrap_err().contains("at least 8"));
    assert!(s.last_result.starts_with("failed: ") && s.last_run == 1_800_000_000 + DAY, "a failure waits for the next slot, it does not retry every minute");
    s.passphrase_file = root.join("nope.txt").display().to_string();
    assert!(run(&l, &mut s, 1_800_000_000 + 2 * DAY, &Progress::default()).unwrap_err().contains("cannot be read"));
    s.passphrase_file = String::new();
    s.dest = root.join("unplugged").display().to_string();
    assert!(run(&l, &mut s, 1_800_000_000 + 3 * DAY, &Progress::default()).unwrap_err().contains("is not there"));
    // stopped: no file left
    let stop = Progress::default();
    stop.cancel.store(true, Ordering::Relaxed);
    s.dest = dest.display().to_string();
    let before = fs::read_dir(&dest).unwrap().count();
    assert!(run(&l, &mut s, 1_800_000_000 + 4 * DAY, &stop).unwrap_err().contains("stopped"));
    assert!(was_stopped(&stop) && fs::read_dir(&dest).unwrap().count() == before);
}
