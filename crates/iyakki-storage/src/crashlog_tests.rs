use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-crashlog-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sess(id: &str, started: i64) -> Session {
    Session { boot_id: id.into(), started, cmdline: "quiet iyakki.ssh=1 BOOT_IMAGE=/boot/bzImage".into(), clean: false }
}

#[test]
fn records_are_parsed_and_urgent_lines_are_recognised() {
    let r = parse_record("3,812,5140900,-;ata1: link is slow\n SUBSYSTEM=ata\n").unwrap();
    assert_eq!((r.level, r.seq, r.ts_us, r.text.as_str()), (3, 812, 5_140_900, "ata1: link is slow"));
    assert_eq!(parse_record("30,1,2,-;x").unwrap().level, 6, "facility bits are dropped: 30 = daemon, info");
    assert_eq!(parse_record("garbage"), None);
    assert!(is_urgent(3, "anything") && is_urgent(2, "x") && !is_urgent(6, "usb 2-1: new device"));
    for w in ["BUG: unable to handle", "Oops: 0002", "Kernel panic - not syncing", "Call Trace:", "RIP: 0010:foo", "Out of memory: Killed", "INFO: task hung task", "watchdog: soft lockup", "foo[12]: segfault at 0"] {
        assert!(is_urgent(6, w), "{w}");
    }
    assert!(has_crash_marker("a\nKernel panic - not syncing\n") && !has_crash_marker("all fine\nusb new device\n"));
}

#[test]
fn lines_get_a_wall_clock_only_when_the_clock_is_plausible() {
    let r = Record { level: 4, seq: 1, ts_us: 12_345_678, text: "hello".into() };
    assert_eq!(format_line(&r, None, 20_000_000), "[   12.345678] <4> hello");
    // now = 2026-10-23 16:00:20 UTC with the kernel at 20 s; the record is 7.65 s older (whole seconds)
    let now = 1_792_771_220;
    assert_eq!(wall(now), "2026-10-23 16:00:20");
    assert_eq!(format_line(&r, Some(now), 20_000_000), "2026-10-23 16:00:13 [   12.345678] <4> hello");
}

#[test]
fn the_queue_is_bounded_drops_the_oldest_and_says_so() {
    let mut q = Queue::new(3);
    for i in 0..5 {
        q.push(format!("l{i}"), false);
    }
    assert_eq!((q.len(), q.dropped), (3, 2));
    let t = q.take();
    assert!(t.starts_with("-- 2 kernel log lines were dropped") && t.ends_with("l2\nl3\nl4\n"), "{t}");
    assert!(q.is_empty() && q.dropped == 0);
}

#[test]
fn nothing_is_written_while_the_drive_is_not_healthy_and_lines_wait_bounded() {
    let mut q = Queue::new(2);
    q.push("a".into(), true);
    assert!(matches!(plan_flush(&mut q, false, true), Flush::Nothing), "an unhealthy drive is never written to, even for an urgent line");
    q.push("b".into(), false);
    q.push("c".into(), false);
    assert_eq!((q.len(), q.dropped), (2, 1), "bounded while waiting");
    q.urgent = false;
    assert!(matches!(plan_flush(&mut q, true, false), Flush::Nothing), "neither due nor urgent");
    assert!(matches!(plan_flush(&mut q, true, true), Flush::Append(t) if t.contains("dropped") && t.contains("c")));
}

#[test]
fn the_reader_never_waits_for_the_disk_and_the_writer_follows_the_watcher() {
    let d = tmp("stall");
    let sh = Shared::new(d.join("kmsg-x.log"));
    let healthy = AtomicBool::new(false);
    let hf = || healthy.load(Ordering::SeqCst);
    let write = |p: &Path, t: &str| append_synced(p, t);
    // 10 000 records arrive while the drive is lost: feeding stays instant and bounded
    let t0 = std::time::Instant::now();
    for i in 0..10_000u32 {
        sh.feed(&format!("6,{i},{},-;line {i}", i as u64 * 1000), None, 10_000_000);
    }
    assert!(t0.elapsed() < Duration::from_secs(2));
    assert!(!sh.round(Duration::from_millis(1), &hf, &write), "lost drive: nothing written");
    assert!(!sh.path.exists());
    healthy.store(true, Ordering::SeqCst);
    assert!(sh.round(Duration::from_millis(1), &hf, &write));
    let text = std::fs::read_to_string(&sh.path).unwrap();
    assert!(text.starts_with("-- 5904 kernel log lines were dropped") && text.contains("line 9999") && !text.contains("line 0\n"), "{}", &text[..200.min(text.len())]);
    assert_eq!(text.lines().count(), 1 + QUEUE_CAP);
    // a failing disk drops what it was given and counts it
    let bad = |_: &Path, _: &str| -> std::io::Result<()> { Err(std::io::Error::other("stalled")) };
    sh.feed("6,1,1,-;x", None, 0);
    assert!(!sh.round(Duration::from_millis(1), &hf, &bad));
    sh.feed("6,2,2,-;y", None, 0);
    assert!(sh.round(Duration::from_millis(1), &hf, &write));
    assert!(std::fs::read_to_string(&sh.path).unwrap().contains("1 kernel log lines were dropped"));
    // an urgent line wakes the writer at once instead of after the wait
    sh.feed("2,3,3,-;Kernel panic - not syncing", None, 0);
    let t0 = std::time::Instant::now();
    assert!(sh.round(Duration::from_secs(30), &hf, &write) && t0.elapsed() < Duration::from_secs(5));
    assert!(std::fs::read_to_string(&sh.path).unwrap().contains("Kernel panic"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn one_boots_file_stops_at_eight_megabytes() {
    let d = tmp("cap");
    let p = d.join("kmsg-x.log");
    std::fs::write(&p, vec![b'a'; (MAX_FILE_BYTES - 10) as usize]).unwrap();
    append_synced(&p, "0123456789ABCDEF\n").unwrap();
    let len = std::fs::metadata(&p).unwrap().len();
    assert!(len > MAX_FILE_BYTES && len < MAX_FILE_BYTES + 100);
    append_synced(&p, "more\n").unwrap();
    assert_eq!(std::fs::metadata(&p).unwrap().len(), len, "no more is written");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_session_json_round_trips_with_awkward_text() {
    let s = Session { boot_id: "ab12".into(), started: 1_792_771_000, cmdline: "a \"quoted\" back\\slash\tTAB".into(), clean: true };
    let back = Session::parse(&s.to_json()).unwrap();
    assert_eq!((back.boot_id.as_str(), back.started, back.clean), ("ab12", 1_792_771_000, true));
    assert!(back.cmdline.starts_with("a \"quoted\" back\\slash"));
    let u = Unclean { boot_id: "z".into(), started: 5, last_line: "[ 3.0] <6> \"x\"".into(), uptime_s: 3, file: "kmsg-crash-z.log".into(), audited: false, seen: true, marker: true };
    assert_eq!(Unclean::parse(&u.to_json()), Some(u));
    assert_eq!(Session::parse("not json"), None);
}

#[test]
fn a_session_that_did_not_end_cleanly_is_detected_kept_and_reported_once() {
    let d = tmp("unclean");
    // boot 1 writes its log and dies (no clean marker)
    assert_eq!(begin_session(&d, &sess("boot0001", 1_000)), None, "no earlier session");
    std::fs::write(d.join("kmsg-boot0001.log"), "[    1.000000] <6> start\n[   99.500000] <6> snd_hda: something\n").unwrap();
    // boot 2
    let u = begin_session(&d, &sess("boot0002", 2_000)).expect("unclean");
    assert_eq!((u.boot_id.as_str(), u.uptime_s, u.file.as_str()), ("boot0001", 99, "kmsg-crash-boot0001.log"));
    assert!(u.last_line.contains("snd_hda") && !u.audited && !u.seen);
    assert!(d.join("kmsg-crash-boot0001.log").exists() && !d.join("kmsg-boot0001.log").exists());
    let lines = std::cell::RefCell::new(Vec::new());
    audit_pending(&d, &|e, det| lines.borrow_mut().push((e.to_string(), det.to_string())));
    audit_pending(&d, &|e, det| lines.borrow_mut().push((e.to_string(), det.to_string())));
    let l = lines.borrow();
    assert_eq!(l.len(), 1, "audited once");
    assert_eq!(l[0].0, "system.unclean-shutdown");
    assert!(l[0].1.contains("snd_hda") && l[0].1.contains("99 s") && l[0].1.contains("kmsg-crash-boot0001.log"), "{}", l[0].1);
    assert!(!read_unclean(&d).unwrap().seen);
    mark_seen(&d);
    assert!(read_unclean(&d).unwrap().seen);
    // boot 2 ends cleanly: boot 3 finds nothing new and shifts the file to previous-1
    std::fs::write(d.join("kmsg-boot0002.log"), "[ 1.0] <6> fine\n").unwrap();
    mark_clean(&d);
    assert_eq!(begin_session(&d, &sess("boot0003", 3_000)), None);
    assert!(d.join("kmsg-previous-1.log").exists() && d.join("kmsg-crash-boot0001.log").exists());
    let hist = std::fs::read_to_string(d.join(SESSIONS_FILE)).unwrap();
    assert!(hist.contains("boot0001\t1000\tunclean\t99") && hist.contains("boot0002\t2000\tclean\t"), "{hist}");
    let txt = sessions_text(&d, Some(&sess("boot0003", 3_000)));
    assert!(txt.contains("unclean") && txt.contains("running") && txt.contains("1970-01-01"), "{txt}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_clean_boot_that_printed_a_panic_marker_keeps_its_file_as_a_crash_file_without_the_alarm() {
    let d = tmp("marker");
    begin_session(&d, &sess("aaaa0001", 10));
    std::fs::write(d.join("kmsg-aaaa0001.log"), "[ 5.0] <6> x\n[ 6.0] <0> Kernel panic - not syncing: fatal\n").unwrap();
    mark_clean(&d);
    assert_eq!(begin_session(&d, &sess("aaaa0002", 20)), None);
    assert!(d.join("kmsg-crash-aaaa0001.log").exists(), "kept until the owner clears it");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rotation_keeps_three_earlier_boots_and_every_crash_file() {
    let d = tmp("rot");
    let mut prev = None::<String>;
    for i in 1..=6 {
        let id = format!("boot000{i}");
        begin_session(&d, &sess(&id, i as i64 * 100));
        std::fs::write(d.join(format!("kmsg-{id}.log")), format!("[ 1.0] <6> boot {i}\n")).unwrap();
        mark_clean(&d);
        prev = Some(id);
    }
    begin_session(&d, &sess("boot0007", 700));
    let names: Vec<String> = dir_files(&d).into_iter().map(|f| f.0).collect();
    assert_eq!(names, ["kmsg-previous-1.log", "kmsg-previous-2.log", "kmsg-previous-3.log"], "{names:?} ({prev:?})");
    assert!(std::fs::read_to_string(d.join("kmsg-previous-1.log")).unwrap().contains("boot 6"));
    assert!(std::fs::read_to_string(d.join("kmsg-previous-3.log")).unwrap().contains("boot 4"));
    // a crash file survives any number of later boots
    std::fs::write(d.join("kmsg-crash-old.log"), "[ 1.0] <0> Kernel panic\n").unwrap();
    for i in 8..=12 {
        let id = format!("boot{i:04}");
        std::fs::write(d.join(format!("kmsg-boot{:04}.log", i - 1)), "[ 1.0] <6> x\n").unwrap();
        mark_clean(&d);
        begin_session(&d, &sess(&id, i as i64 * 100));
    }
    assert!(d.join("kmsg-crash-old.log").exists());
    assert!(dir_files(&d).iter().filter(|f| f.0.starts_with("kmsg-previous-")).count() <= KEEP_PREVIOUS);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_folder_is_bounded_dropping_old_normal_files_before_crash_files() {
    let d = tmp("bound");
    let big = vec![b'x'; 1000];
    for (n, age) in [("kmsg-previous-1.log", 3u64), ("kmsg-previous-2.log", 2), ("kmsg-crash-a.log", 1), ("kmsg-crash-b.log", 4), ("kmsg-previous-3.log", 1)] {
        std::fs::write(d.join(n), &big).unwrap();
        let _ = age;
    }
    let ages = |n: &str| match n {
        "kmsg-previous-1.log" => 3,
        "kmsg-previous-2.log" => 2,
        "kmsg-previous-3.log" => 1,
        "kmsg-crash-a.log" => 1,
        _ => 4,
    };
    let gone = enforce_dir_bound(&d, 3500, &ages);
    assert_eq!(gone, ["kmsg-previous-3.log", "kmsg-previous-2.log"], "oldest normal files first, crash files stay");
    let gone = enforce_dir_bound(&d, 1500, &ages);
    assert_eq!(gone, ["kmsg-previous-1.log", "kmsg-crash-a.log"], "crash files only when no normal file is left");
    assert!(d.join("kmsg-crash-b.log").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_command_line_tool_prints_the_files() {
    let data = tmp("cli");
    let dir = data.join(LOG_SUBDIR);
    begin_session(&dir, &sess("cur00001", 50));
    std::fs::write(dir.join("kmsg-cur00001.log"), "[ 1.0] <6> now\n").unwrap();
    std::fs::write(dir.join("kmsg-previous-1.log"), "[ 1.0] <6> before\n").unwrap();
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(cli(&a(&["kernel"]), &data, "cur00001").0.contains("now"));
    assert!(cli(&a(&["kernel", "--previous"]), &data, "cur00001").0.contains("before"));
    let (t, ok) = cli(&a(&["kernel", "--crash"]), &data, "cur00001");
    assert!(ok && t.contains("No crash log"));
    std::fs::write(dir.join("kmsg-crash-z.log"), "[ 1.0] <0> boom\n").unwrap();
    assert!(cli(&a(&["kernel", "--crash"]), &data, "cur00001").0.contains("boom"));
    assert!(cli(&a(&["sessions"]), &data, "cur00001").0.contains("running"));
    assert!(!cli(&a(&["nonsense"]), &data, "x").1);
    let (t, ok) = cli(&a(&["kernel", "--previous"]), &tmp("empty"), "x");
    assert!(!ok && t.contains("kmsg-previous-1.log"));
    let _ = std::fs::remove_dir_all(&data);
}

