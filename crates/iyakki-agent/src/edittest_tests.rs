//! Tests of the editor's test runner: what is planned, how a run is counted, a stop, a time limit and output that is not text.
//! Fake programs are small shell scripts in a temporary folder; no cargo or pytest is needed.

// the boundary scan reads a file's code up to its first #[cfg(test)]: this whole file is test code (it runs small scripts)
#[cfg(test)]
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::edittest::*;

fn tmp(tag: &str) -> PathBuf {
    // outside every Cargo project and git work tree: a lookup that walks up the parents must find nothing of the repository (the gate's TMPDIR is in it)
    crate::testdir::outside_project("iyakki-edtest", tag)
}

fn script(dir: &Path, body: &str) -> TestPlan {
    TestPlan { label: "fake".into(), program: "sh".into(), args: vec!["-c".into(), body.into()], cwd: dir.into() }
}

#[test]
fn a_rust_file_plans_cargo_test_for_its_project_and_a_tests_file_runs_just_its_binary() {
    let d = tmp("plan-rs");
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::create_dir_all(d.join("tests")).unwrap();
    std::fs::write(d.join("Cargo.toml"), "[package]\n").unwrap();
    let p = plan(&d.join("src/lib.rs")).unwrap();
    assert_eq!((p.program.as_str(), p.cwd.as_path()), ("cargo", d.as_path()));
    assert_eq!(p.args[0], "test");
    assert!(!p.args.contains(&"--test".to_string()));
    let p = plan(&d.join("tests/api.rs")).unwrap();
    assert!(p.args.windows(2).any(|w| w == ["--test", "api"]), "{:?}", p.args);
    let lonely = tmp("plan-lonely");
    assert!(plan(&lonely.join("a.rs")).unwrap_err().contains("Cargo project"));
}

#[test]
fn a_python_test_file_plans_pytest_in_its_project_and_other_files_are_refused() {
    let d = tmp("plan-py");
    std::fs::create_dir_all(d.join("pkg")).unwrap();
    std::fs::write(d.join("pyproject.toml"), "").unwrap();
    let p = plan(&d.join("pkg/test_a.py")).unwrap();
    assert_eq!((p.program.as_str(), p.cwd.as_path()), ("python3", d.as_path()));
    assert!(p.args.starts_with(&["-m".to_string(), "pytest".to_string()]));
    assert!(plan(&d.join("pkg/b_test.py")).is_ok());
    assert!(plan(&d.join("pkg/main.py")).unwrap_err().contains("test_*.py"));
    assert!(plan(&d.join("x.txt")).is_err());
}

#[test]
fn cargo_and_pytest_output_is_counted_with_the_failed_names() {
    let cargo = b"running 3 tests\ntest a::one ... ok\ntest a::two ... FAILED\ntest a::three ... ignored\n\nfailures:\n    a::two\n\ntest result: FAILED. 1 passed; 1 failed; 1 ignored\n";
    let r = summarize(cargo, Some(101));
    assert_eq!((r.ok, r.passed, r.failed, r.skipped), (false, 1, 1, 1));
    assert_eq!(r.failures, vec!["a::two".to_string()]);
    let t = r.text("cargo test");
    assert!(t.starts_with("cargo test: 1 passed, 1 failed, 1 skipped.") && t.contains("FAILED a::two") && t.contains("test result: FAILED"), "{t}");
    let py = b"test_a.py::test_x PASSED  [ 50%]\ntest_a.py::test_y PASSED [100%]\n== 2 passed in 0.01s ==\n";
    let r = summarize(py, Some(0));
    assert_eq!((r.ok, r.passed, r.failed), (true, 2, 0));
    assert!(!r.text("pytest").contains("FAILED"));
    let none = summarize(b"error: could not compile\n", Some(101));
    assert!(none.text("cargo test").contains("no test results found (exit 101)"));
}

#[test]
fn a_run_reports_its_counts_and_a_missing_program_says_so() {
    let d = tmp("run");
    let p = script(&d, "echo 'test x::a ... ok'; echo 'test x::b ... ok'; echo 'test x::c ... FAILED' >&2; exit 101");
    let r = run(&p, 30, &AtomicBool::new(false)).unwrap();
    assert_eq!((r.ok, r.passed, r.failed, r.exit), (false, 2, 1, Some(101)));
    let missing = TestPlan { program: "iyakki-no-such-program".into(), ..p };
    assert!(run(&missing, 5, &AtomicBool::new(false)).unwrap_err().contains("not installed"));
}

#[test]
fn stop_really_ends_the_run_and_a_time_limit_does_too() {
    let d = tmp("stop");
    let p = script(&d, "echo 'test x::a ... ok'; sleep 30");
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    let h = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        f2.store(true, Ordering::SeqCst);
    });
    let t0 = Instant::now();
    let r = run(&p, 60, &flag).unwrap();
    h.join().unwrap();
    assert!(r.stopped && !r.ok && t0.elapsed() < Duration::from_secs(10), "{r:?}");
    assert!(r.text("fake").starts_with("fake: stopped."));
    let t0 = Instant::now();
    let r = run(&p, 5, &AtomicBool::new(false)).unwrap();
    assert!(r.timed_out && t0.elapsed() < Duration::from_secs(20));
    assert!(r.text("fake").contains("ran out of time"));
}

#[test]
fn output_that_is_not_text_is_described_not_dumped() {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&[0, 0, 2, 128, 0, 0, 1, 224, 8, 6]);
    let d = describe_bytes(&png);
    assert!(d.starts_with("[not text: PNG picture, 640 x 480, ") && d.contains("starts 89 50 4e 47"), "{d}");
    assert!(describe_bytes(b"%PDF-1.7 \0").contains("PDF document"));
    assert!(describe_bytes(b"PK\x03\x04abc").contains("ZIP archive"));
    assert!(describe_bytes(&[0xff, 0xd8, 0xff, 0xe0]).contains("JPEG"));
    assert!(describe_bytes(&[1, 2, 3, 0]).contains("binary data, 4 bytes"));
    assert_eq!(text_of("plain é text\n".as_bytes()), "plain é text\n");
    assert!(text_of(&png).starts_with("[not text:"));
    // a run whose output is binary gives a description in the tail, not garbage
    let d = tmp("bin");
    let r = run(&script(&d, "printf '\\000\\001\\002\\003'; exit 3"), 30, &AtomicBool::new(false)).unwrap();
    assert!(r.tail.contains("not text: binary data"), "{}", r.tail);
}

#[test]
fn a_test_run_for_a_person_is_claimed_while_it_runs() {
    let d = tmp("claim");
    let who = format!("edclaim-{}", std::process::id());
    let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (s2, who2) = (seen.clone(), who.clone());
    let watcher = std::thread::spawn(move || {
        for _ in 0..300 {
            if crate::procowner::claims().iter().any(|c| c.user == who2 && c.what == "fake") {
                s2.store(true, Ordering::SeqCst);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
    let r = crate::procowner::as_user(&who, || run(&script(&d, "sleep 1; echo 'test x::a ... ok'"), 30, &AtomicBool::new(false)));
    watcher.join().unwrap();
    assert!(r.unwrap().ok);
    assert!(seen.load(Ordering::SeqCst), "the run was listed under the person while it ran");
    assert!(!crate::procowner::claims().iter().any(|c| c.user == who), "and the claim ended with it");
    // no person set: nothing is claimed
    assert!(run(&script(&d, "echo 'test x::a ... ok'"), 30, &AtomicBool::new(false)).unwrap().ok);
}
