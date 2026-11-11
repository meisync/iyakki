//! Safe graphics mode with fakes only: the decision table, the memory per card, the files, the environment.

use crate::webgraphics::*;
use std::path::PathBuf;

const GPU: &str = "1002:1681";

/// The pending-notice queue is one static: the tests that read it take turns.
fn pending_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
const T0: i64 = 1_793_980_000; // the previous session started
const HW: &str = "graphics: hardware AMD Radeon 610M (radeonsi, raphael_mendocino, ACO, DRM 3.61, 6.12.112) on /dev/dri/renderD128 (BGRA read-back)";

fn prev(unclean: bool, uptime: u64) -> Prev {
    Prev { boot_id: "aaaa1111".into(), started: T0, unclean, uptime_s: uptime }
}
fn hw_log() -> Vec<(i64, String)> {
    vec![(T0 + 100, "Mesa: cache disabled".into()), (T0 + 116, HW.into())]
}
fn run(mode: Mode, p: Option<&Prev>, log: &[(i64, String)], st: &State, no_driver: bool) -> Decision {
    decide(&Inputs { mode, gpu: GPU, no_driver, prev: p, cur_started: T0 + 5000, log, state: st, now: T0 + 5010 })
}

#[test]
fn the_decision_table() {
    let st = State::default();
    let hw = hw_log();
    let none: Vec<(i64, String)> = vec![(T0 + 50, "sound: x".into())];
    let sw_run = vec![(T0 + 116, "graphics: software requested, using shared memory pictures".to_string())];
    // (name, mode, previous session, log, state, software, why)
    let c_clean = prev(false, 3000);
    let c_freeze = prev(true, 130); // last kernel line 130 s after the start; the engine started at +116
    let rows: Vec<(&str, Mode, Option<Prev>, &Vec<(i64, String)>, State, bool, Why)> = vec![
        ("automatic, clean end after a hardware start", Mode::Automatic, Some(c_clean.clone()), &hw, st.clone(), false, Why::Normal),
        ("automatic, unclean after a hardware start", Mode::Automatic, Some(c_freeze.clone()), &hw, st.clone(), true, Why::NewFreeze),
        ("automatic, unclean, no engine start", Mode::Automatic, Some(c_freeze.clone()), &none, st.clone(), false, Why::Normal),
        ("automatic, unclean, the engine ran in software", Mode::Automatic, Some(c_freeze.clone()), &sw_run, st.clone(), false, Why::Normal),
        ("automatic, no history at all", Mode::Automatic, None, &hw, st.clone(), false, Why::Normal),
        ("software setting, clean", Mode::Software, Some(c_clean.clone()), &hw, st.clone(), true, Why::ChosenSoftware),
        ("software setting, nothing known", Mode::Software, None, &none, st.clone(), true, Why::ChosenSoftware),
        ("hardware setting, clean", Mode::Hardware, Some(c_clean.clone()), &hw, st.clone(), false, Why::ChosenHardware),
        ("hardware setting, even after a freeze", Mode::Hardware, Some(c_freeze.clone()), &hw, st.clone(), false, Why::ChosenHardware),
    ];
    for (name, mode, p, log, state, software, why) in rows {
        let d = run(mode, p.as_ref(), log, &state, false);
        assert_eq!((d.software, d.why), (software, why), "{name}");
        assert!(d.log_line.starts_with(if software { "graphics mode: software" } else { "graphics mode: hardware" }), "{name}: {}", d.log_line);
    }
}

#[test]
fn a_freeze_after_a_hardware_start_falls_back_with_the_reason_and_the_plain_words() {
    let d = run(Mode::Automatic, Some(&prev(true, 130)), &hw_log(), &State::default(), false);
    assert_eq!(d.notice.as_deref(), Some("The last session stopped right after the browser started on this graphics card, so the browser now draws in software (slower). Settings, Browser, Graphics changes this."));
    assert!(d.audit.as_deref().unwrap().contains("1002:1681") && d.audit.as_deref().unwrap().contains("aaaa1111"));
    assert!(d.log_line.contains("stopped right after the engine started"), "{}", d.log_line);
    assert_eq!(d.state.fallbacks.len(), 1);
    assert_eq!(d.state.fallbacks[0].gpu, GPU);
    assert_eq!(d.state.judged_boot, "aaaa1111");
}

#[test]
fn a_clean_shutdown_a_power_cut_without_an_engine_and_a_late_stop_never_trigger_it() {
    let none: Vec<(i64, String)> = vec![];
    for (name, p, log) in [
        ("clean shutdown", prev(false, 3000), hw_log()),
        ("clean with errors counts as clean", prev(false, 10), hw_log()),
        ("power cut, the engine never started", prev(true, 900), none.clone()),
        ("power cut after a long quiet hour", prev(true, 4000), hw_log()),
    ] {
        let d = run(Mode::Automatic, Some(&p), &log, &State::default(), false);
        assert!(!d.software && d.notice.is_none() && d.state.fallbacks.is_empty(), "{name}");
    }
    // an engine start in an OLDER session is not this session's
    let old = vec![(T0 - 3000, HW.to_string())];
    assert!(!run(Mode::Automatic, Some(&prev(true, 100)), &old, &State::default(), false).software);
    // a start after the previous session ended belongs to the running session
    let later = vec![(T0 + 6000, HW.to_string())];
    assert!(!run(Mode::Automatic, Some(&prev(true, 100)), &later, &State::default(), false).software);
    // a clock that was not set (time before 2025) gives no evidence
    let mut p = prev(true, 100);
    p.started = 1000;
    assert!(!run(Mode::Automatic, Some(&p), &[(1100, HW.to_string())], &State::default(), false).software);
}

#[test]
fn the_engine_log_after_the_start_counts_as_a_sign_of_life() {
    // the kernel log stopped early, but the engine kept writing for an hour: not "right after"
    let mut log = hw_log();
    log.push((T0 + 3700, "helper: v4l2src busy".into()));
    assert!(!run(Mode::Automatic, Some(&prev(true, 130)), &log, &State::default(), false).software);
}

#[test]
fn hardware_setting_never_falls_back_but_says_what_happened() {
    let d = run(Mode::Hardware, Some(&prev(true, 130)), &hw_log(), &State::default(), false);
    assert!(!d.software);
    assert_eq!(d.notice.as_deref(), Some(FREEZE_NOTICE_HARDWARE));
    assert!(d.state.fallbacks.is_empty(), "nothing is remembered against the person's choice");
    assert!(d.log_line.contains("the choice is kept") && d.audit.is_some());
    // said once: the same session is not judged again
    let again = run(Mode::Hardware, Some(&prev(true, 130)), &hw_log(), &d.state, false);
    assert!(again.notice.is_none() && again.audit.is_none());
    // the missing Intel driver is a fact, not a fall back
    assert!(run(Mode::Hardware, None, &[], &State::default(), true).software);
}

#[test]
fn the_fall_back_is_remembered_per_card_and_stable_within_a_boot() {
    let first = run(Mode::Automatic, Some(&prev(true, 130)), &hw_log(), &State::default(), false);
    // a second engine start in the same boot (media, assistant browser): same answer, no second notice
    let second = run(Mode::Automatic, Some(&prev(true, 130)), &hw_log(), &first.state, false);
    assert!(second.software && second.why == Why::Remembered && second.notice.is_none() && second.audit.is_none());
    // another card is not affected
    let other = decide(&Inputs { mode: Mode::Automatic, gpu: "8086:3ea5", no_driver: false, prev: None, cur_started: T0 + 5000, log: &[], state: &first.state, now: T0 + 6000 });
    assert!(!other.software);
    // next boot, the software session ended unexpectedly (a power cut): still software, no offer yet
    let p2 = Prev { boot_id: "bbbb2222".into(), started: T0 + 5000, unclean: true, uptime_s: 90 };
    let third = decide(&Inputs { mode: Mode::Automatic, gpu: GPU, no_driver: false, prev: Some(&p2), cur_started: T0 + 9000, log: &[], state: &first.state, now: T0 + 9010 });
    assert!(third.software && third.notice.is_none() && !third.state.offer);
}

#[test]
fn a_clean_session_in_software_offers_hardware_once_and_never_forces_it() {
    let st = run(Mode::Automatic, Some(&prev(true, 130)), &hw_log(), &State::default(), false).state;
    let p2 = Prev { boot_id: "bbbb2222".into(), started: T0 + 5000, unclean: false, uptime_s: 4000 };
    let sw = vec![(T0 + 5100, "graphics: software requested, using shared memory pictures".to_string())];
    let mk = |s: &State| decide(&Inputs { mode: Mode::Automatic, gpu: GPU, no_driver: false, prev: Some(&p2), cur_started: T0 + 20000, log: &sw, state: s, now: T0 + 20010 });
    let d = mk(&st);
    assert!(d.software, "offered, not forced");
    assert_eq!(d.notice.as_deref(), Some(OFFER_NOTICE));
    assert!(d.state.offer);
    assert!(mk(&d.state).notice.is_none(), "offered once per session");
}

#[test]
fn a_later_hardware_start_that_ends_cleanly_clears_the_fall_back() {
    let st = run(Mode::Automatic, Some(&prev(true, 130)), &hw_log(), &State::default(), false).state;
    // the person chose Hardware without pressing anything else; that session started on the card and ended cleanly
    let p2 = Prev { boot_id: "cccc3333".into(), started: T0 + 5000, unclean: false, uptime_s: 4000 };
    let log = vec![(T0 + 5100, HW.to_string())];
    let d = decide(&Inputs { mode: Mode::Hardware, gpu: GPU, no_driver: false, prev: Some(&p2), cur_started: T0 + 20000, log: &log, state: &st, now: T0 + 20010 });
    assert!(d.state.fallbacks.is_empty() && !d.software);
    assert!(d.audit.unwrap().contains("cleared"));
    // and Automatic then uses the card
    let a = decide(&Inputs { mode: Mode::Automatic, gpu: GPU, no_driver: false, prev: Some(&p2), cur_started: T0 + 20000, log: &log, state: &d.state, now: T0 + 20020 });
    assert!(!a.software && a.why == Why::Normal);
}

#[test]
fn no_gpu_and_no_driver_cases() {
    let d = decide(&Inputs { mode: Mode::Automatic, gpu: "", no_driver: false, prev: Some(&prev(true, 130)), cur_started: T0 + 5000, log: &hw_log(), state: &State::default(), now: T0 + 5010 });
    assert!(!d.software && d.why == Why::NoGpu && d.state.fallbacks.is_empty());
    let d = run(Mode::Automatic, None, &[], &State::default(), true);
    assert!(d.software && d.why == Why::NoDriver);
}

#[test]
fn the_environment_of_the_software_path() {
    let e = software_env(8, &[]);
    assert_eq!(e, vec![("LIBGL_ALWAYS_SOFTWARE".to_string(), "1".to_string()), ("LP_NUM_THREADS".to_string(), "4".to_string())]);
    let tuned = vec![("LP_NUM_THREADS".to_string(), "2".to_string())];
    assert_eq!(software_env(8, &tuned), vec![("LIBGL_ALWAYS_SOFTWARE".to_string(), "1".to_string())], "the web-tuning word wins");
    assert_eq!(gpu_id(&[("0x1002\n".into(), "0x1681\n".into())]), "1002:1681");
    assert_eq!(gpu_id(&[("0x10de".into(), "0x2560".into()), ("0x1002".into(), "0x1681".into()), ("0x1002".into(), "0x1681".into())]), "1002:1681+10de:2560");
    assert_eq!(gpu_id(&[]), "");
}

// ------------------------------------------------------------------------------------------------ files

fn data(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-webgfx-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("web-profile")).unwrap();
    d
}

fn world(d: &std::path::Path, p: Option<Prev>, log: Vec<(i64, String)>) -> World {
    World { gpus: vec![("0x1002\n".into(), "0x1681\n".into())], marker: String::new(), prev: p, cur_started: T0 + 5000, log, log_dir: d.join("log"), now: T0 + 5010 }
}

#[test]
fn the_setting_is_saved_versioned_and_changing_it_forgets_the_fall_back() {
    let _turn = pending_lock(); // apply_with queues a notice in a process-wide list: tests that call it take turns
    let d = data("set");
    assert_eq!(load_mode(&d), (Mode::Automatic, None));
    let w = world(&d, Some(prev(true, 130)), hw_log());
    assert!(apply_with(&d, &w).software);
    assert_eq!(load_state(&d).fallbacks.len(), 1);
    set_mode(&d, Mode::Hardware).unwrap();
    assert_eq!(load_mode(&d).0, Mode::Hardware);
    assert!(load_state(&d).fallbacks.is_empty(), "the person decided");
    let text = std::fs::read_to_string(d.join("web-profile").join(SETTING_FILE)).unwrap();
    assert!(text.contains("\"schema\": 1") && text.contains("hardware"), "{text}");
    set_mode(&d, Mode::Automatic).unwrap();
    // the same old freeze is not judged a second time after the person's decision
    assert!(!apply_with(&d, &world(&d, Some(prev(true, 130)), hw_log())).software, "judged already");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn apply_writes_the_engine_log_the_audit_note_and_the_toast_once() {
    let _turn = pending_lock();
    let d = data("apply");
    let _ = take_pending();
    let w = world(&d, Some(prev(true, 130)), hw_log());
    let dec = apply_with(&d, &w);
    assert!(dec.software);
    let log = std::fs::read_to_string(d.join("log").join("engine.log")).unwrap();
    assert!(log.contains("graphics mode: software") && log.contains("stopped right after the engine started"), "{log}");
    assert!(log.contains("Browser graphics: software from now on for card 1002:1681"), "{log}");
    let q = take_pending();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].status.as_deref(), Some(FREEZE_NOTICE));
    assert!(q[0].toast.as_deref().unwrap().len() < 100 && q[0].toast.as_deref().unwrap().contains("software"), "the toast is one short line");
    assert!(q[0].audit.as_deref().unwrap().contains("1002:1681"));
    // the second start of the same boot: logged, but nothing new to say
    let _ = apply_with(&d, &w);
    assert!(take_pending().is_empty());
    assert_eq!(status_with(&d, &w.gpus, "").line, "Automatic: software because the last session stopped right after the browser started on this graphics card");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_damaged_or_newer_setting_file_is_moved_aside_or_left_alone_and_never_overwritten() {
    let d = data("bad");
    let f = d.join("web-profile").join(SETTING_FILE);
    std::fs::write(&f, "{ half").unwrap();
    assert_eq!(load_mode(&d), (Mode::Automatic, None), "defaults");
    let moved: Vec<_> = std::fs::read_dir(d.join("web-profile")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".corrupt-")).collect();
    assert_eq!(moved.len(), 1, "the damaged bytes were moved aside");
    assert_eq!(std::fs::read_to_string(moved[0].path()).unwrap(), "{ half");
    // newer than this release: read, never rewritten
    std::fs::write(&f, "{\"schema\": 9, \"mode\": \"software\"}").unwrap();
    let (m, ro) = load_mode(&d);
    assert_eq!(m, Mode::Software);
    assert!(ro.is_some());
    assert!(set_mode(&d, Mode::Hardware).is_err());
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "{\"schema\": 9, \"mode\": \"software\"}");
    assert!(status_with(&d, &[], "").read_only.is_some());
    // the memory file too
    let s = d.join("web-profile").join(STATE_FILE);
    std::fs::write(&s, "not json").unwrap();
    assert!(load_state(&d).fallbacks.is_empty());
    assert!(std::fs::read_dir(d.join("web-profile")).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("web-graphics-state.json.corrupt-")));
    // an unknown value reads as Automatic
    std::fs::write(&f, "{\"schema\": 1, \"mode\": \"turbo\"}").unwrap();
    assert_eq!(load_mode(&d).0, Mode::Automatic);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn try_hardware_again_forgets_one_card_and_keeps_the_judged_session() {
    let _turn = pending_lock(); // apply_with queues a notice in a process-wide list: tests that call it take turns
    let d = data("retry");
    assert!(try_hardware_again(&d, GPU).is_err(), "nothing to forget");
    let w = world(&d, Some(prev(true, 130)), hw_log());
    assert!(apply_with(&d, &w).software);
    try_hardware_again(&d, GPU).unwrap();
    assert!(!apply_with(&d, &w).software, "the old freeze is not counted twice");
    assert_eq!(status_with(&d, &w.gpus, "").line, "Automatic: hardware");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_status_words_for_each_setting() {
    let d = data("status");
    let gpus = vec![("0x1002".to_string(), "0x1681".to_string())];
    assert_eq!(status_with(&d, &gpus, "").line, "Automatic: hardware");
    set_mode(&d, Mode::Software).unwrap();
    let s = status_with(&d, &gpus, "");
    assert!(s.software && s.line == "Software (safe)");
    set_mode(&d, Mode::Hardware).unwrap();
    let s = status_with(&d, &gpus, "");
    assert!(!s.software && s.line.starts_with("Hardware"));
    // an Intel card with no Intel driver in the image
    let intel = vec![("0x8086".to_string(), "0x3ea5".to_string())];
    assert!(status_with(&d, &intel, "software").software);
    set_mode(&d, Mode::Automatic).unwrap();
    assert!(status_with(&d, &intel, "software").line.contains("no driver"));
    assert!(!status_with(&d, &intel, "intel").software);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_mode_names() {
    for m in [Mode::Automatic, Mode::Software, Mode::Hardware] {
        assert_eq!(Mode::from_id(m.id()), Some(m));
        assert_eq!(Mode::from_id(m.label()), Some(m));
    }
    assert_eq!(Mode::Automatic.next().next().next(), Mode::Automatic);
    assert_eq!(Mode::from_id("x"), None);
}

/// KI (HP Pavilion 16-af0, 2026-11-07): the kernel declared the Meteor Lake card wedged (no GuC firmware before the root was mounted);
/// Mesa could not query the device and the helper died by a signal before its first frame. The shell falls back in the same session.
#[test]
fn a_helper_that_dies_right_after_a_hardware_start_falls_back_at_once_but_not_on_a_clean_end_or_a_hardware_setting() {
    let _turn = pending_lock();
    let _ = take_pending();
    let log = vec!["wpe-helper: sound: ALSA_CONFIG_PATH=(unset)".to_string(), "MESA: warning: Could not get intel_device_info.".to_string(), "graphics: hardware llvmpipe (LLVM 21.1.8, 256 bits) on /dev/dri/renderD128 (BGRA read-back)".to_string()];
    let why = "the connection to the helper failed: Connection reset by peer (os error 104) (ended by a signal)";
    assert!(died_at_start(&log, why, false));
    assert!(!died_at_start(&log, why, true), "an engine that was ready and stopped later is a freeze for the next start, not this");
    assert!(!died_at_start(&log, "exited with status 0", false), "a clean exit is not a crash");
    assert!(!died_at_start(&["graphics: software requested, using shared memory pictures".to_string()], why, false), "a crash in software mode has nothing to fall back to");
    let data = std::env::temp_dir().join(format!("iyakki-webgraphics-now-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).unwrap();
    assert!(fall_back_now(&data, "", "x").is_err(), "no card known: nothing to remember");
    fall_back_now(&data, "8086:7d45", "ended by a signal").unwrap();
    assert!(load_state(&data).fallback_for("8086:7d45").is_some(), "the card is remembered for the next start");
    // other tests queue notices through apply_with at the same time: look for ours
    let p = take_pending();
    let ours = p.iter().find(|x| x.status.as_deref() == Some(CRASH_NOTICE)).expect("the crash notice is queued");
    assert!(ours.audit.as_deref().unwrap_or("").contains("8086:7d45"));
    assert!(ours.toast.as_deref().unwrap_or("").len() < 120, "the toast is one short line");
    assert!(fall_back_now(&data, "8086:7d45", "again").is_err(), "a second crash does not restart the engine in a loop");
    // the person chose Hardware: the shell never overrides that
    set_mode(&data, Mode::Hardware).unwrap();
    assert!(fall_back_now(&data, "8086:7d45", "x").is_err());
    let _ = std::fs::remove_dir_all(&data);
}
