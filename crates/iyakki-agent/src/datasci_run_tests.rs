// SPDX-License-Identifier: Apache-2.0
use super::*;
use std::sync::Mutex;

fn tmp(tag: &str) -> PathBuf {
    iyakki_archive::archive::tests::tmp(&format!("datasci-run-{tag}"))
}

/// A launcher that records the arguments and answers from a table.
struct Fake {
    seen: Mutex<Vec<(Vec<String>, Duration)>>,
    answer: Result<NsOutput, String>,
}

impl NsLauncher for Fake {
    fn launch(&self, args: &[String], timeout: Duration, _cap: usize) -> Result<NsOutput, String> {
        self.seen.lock().unwrap().push((args.to_vec(), timeout));
        self.answer.clone()
    }
}

fn out(code: i32, stdout: &str, stderr: &str) -> NsOutput {
    NsOutput { code: Some(code), signal: None, timed_out: false, stdout: stdout.as_bytes().to_vec(), stderr: stderr.as_bytes().to_vec() }
}

/// A toolkit folder as the install leaves it (made-up files).
fn toolkit(d: &Path, files: &[&str]) {
    std::fs::create_dir_all(datasci::dir(d).join("python/bin")).unwrap();
    std::fs::write(datasci::python_bin(d), "#!fake").unwrap();
    std::fs::create_dir_all(datasci::site_dir(d)).unwrap();
    let st = datasci::State { files: files.iter().map(|s| s.to_string()).collect(), selftest: "scikit-learn 1.9.1 fits a small model in 0.6 s".into() };
    std::fs::write(datasci::dir(d).join("state.json"), serde_json::to_string(&st).unwrap()).unwrap();
}

fn flag<'a>(a: &'a [String], f: &str) -> &'a str {
    &a[a.iter().position(|x| x == f).unwrap_or_else(|| panic!("{f} in {a:?}")) + 1]
}

#[test]
fn the_run_has_no_network_the_toolkit_read_only_one_writable_folder_and_every_limit() {
    let d = tmp("argv");
    let sp = spec_for(Path::new("/var/lib/iyakki/linuxapps/amy/debian/rootfs"), &d, &d.join("scratch/run1"), "script.py", Limits::SELFTEST);
    sp.validate().unwrap();
    let a = sp.to_args();
    assert_eq!(a[0], "linuxapp-init");
    assert_eq!(flag(&a, "--net"), "off", "no network, not even a loopback to the computer");
    assert!(!a.contains(&"--veth".to_string()), "no outbound-only network either");
    assert_eq!((flag(&a, "--mem-mb"), flag(&a, "--cpu-pct"), flag(&a, "--pids"), flag(&a, "--tmp-mb"), flag(&a, "--seccomp")), ("1024", "200", "64", "512", "on"));
    assert_eq!((flag(&a, "--uid"), flag(&a, "--gid"), flag(&a, "--idmap"), flag(&a, "--cwd")), ("1000", "1000", "single", "/work"));
    // the mounts: python and the packages read-only, the scratch folder the only writable one
    let binds: Vec<(&str, &str, &str)> = a.windows(3).filter(|w| w[0] == "--bind-ro" || w[0] == "--bind-rw").map(|w| (w[0].as_str(), w[1].as_str(), w[2].as_str())).collect();
    assert_eq!(binds.len(), 3);
    assert!(binds[0].0 == "--bind-ro" && binds[0].1.ends_with("datasci/python") && binds[0].2 == "/opt/datasci/python");
    assert!(binds[1].0 == "--bind-ro" && binds[1].1.ends_with("datasci/site") && binds[1].2 == "/opt/datasci/site");
    assert!(binds[2].0 == "--bind-rw" && binds[2].1.ends_with("scratch/run1") && binds[2].2 == "/work");
    // isolated Python, the packages path put in by the start-up (isolated mode ignores PYTHONPATH), then the script of the work folder
    let at = a.iter().position(|x| x == "--").unwrap();
    assert_eq!(&a[at + 1..at + 4], ["/opt/datasci/python/bin/python3", "-I", "-B"]);
    assert!(a[at + 5].contains("sys.path.insert(0, '/opt/datasci/site')") && a[at + 5].contains("/work/script.py"), "{:?}", &a[at..]);
    // the environment limits the maths libraries' threads and keeps Python from writing files
    for e in ["OMP_NUM_THREADS=2", "OPENBLAS_NUM_THREADS=2", "PYTHONDONTWRITEBYTECODE=1", "HOME=/work"] {
        assert!(a.windows(2).any(|w| w[0] == "--env" && w[1] == e), "{e}");
    }
    assert!(!a.iter().any(|x| x.starts_with("PYTHONPATH") || x.starts_with("PYTHONHOME")), "isolated mode would ignore them");
    // a script name that tries to leave the work folder would be in the start-up text: the callers pass fixed names only
    assert!(command("selftest.py")[4].ends_with("'/work/selftest.py', run_name='__main__')"));
}

#[test]
fn the_self_test_runs_in_a_scratch_folder_that_is_emptied_and_its_answer_is_one_sentence() {
    let d = tmp("ok");
    toolkit(&d, &["numpy 2.5.4 aa"]);
    let l = Fake { seen: Mutex::new(vec![]), answer: Ok(out(0, "noise\nSELFTEST ok scikit-learn 1.9.1 fits a small model in 0.6 s; OpenCV 4.14.0 resizes an image\n", "")) };
    let said = selftest(&l, Path::new("/root/fs"), &d, &d.join("scratch")).unwrap();
    assert_eq!(said, "scikit-learn 1.9.1 fits a small model in 0.6 s; OpenCV 4.14.0 resizes an image");
    let seen = l.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1, Duration::from_secs(SELFTEST_SECS), "the caller enforces the time");
    assert_eq!(flag(&seen[0].0, "--net"), "off");
    assert!(!d.join("scratch/selftest").exists(), "nothing is left behind");
    // the script that ran is the embedded one: it checks the score and resizes an image
    assert!(SELFTEST_SCRIPT.contains("RandomForestClassifier") && SELFTEST_SCRIPT.contains("score <= 0.9") && SELFTEST_SCRIPT.contains("cv2.resize"));
    assert!(SELFTEST_SCRIPT.contains("SELFTEST ok") && SELFTEST_SCRIPT.contains("SELFTEST failed"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_failed_or_silent_or_slow_self_test_says_why_and_a_missing_toolkit_runs_nothing() {
    let d = tmp("bad");
    toolkit(&d, &[]);
    let run = |o: Result<NsOutput, String>| selftest(&Fake { seen: Mutex::new(vec![]), answer: o }, Path::new("/root/fs"), &d, &d.join("scratch"));
    let e = run(Ok(out(1, "SELFTEST failed ImportError: libgfortran.so.5: cannot open shared object file\n", ""))).unwrap_err();
    assert!(e.contains("self-test failed") && e.contains("libgfortran"), "{e}");
    // exit 0 with a failed line, or an ok line with a bad exit code, is not a pass
    assert!(run(Ok(out(0, "SELFTEST failed the model scored only 0.50\n", ""))).is_err());
    assert!(run(Ok(out(3, "SELFTEST ok something\n", ""))).is_err());
    let e = run(Ok(out(125, "", "linuxapp-init: entering the sandbox failed: pivot_root: Invalid argument\n"))).unwrap_err();
    assert!(e.contains("did not run") && e.contains("exit 125") && e.contains("pivot_root"), "{e}");
    let slow = NsOutput { timed_out: true, ..out(0, "", "") };
    assert!(run(Ok(slow)).unwrap_err().contains("did not finish within 120 seconds"));
    let killed = NsOutput { code: None, signal: Some(9), ..out(0, "", "") };
    assert!(run(Ok(killed)).unwrap_err().contains("stopped by a signal"));
    assert_eq!(run(Err("no sandbox here".into())).unwrap_err(), "no sandbox here");
    // control characters in what the toolkit prints never reach the person's screen
    let e = run(Ok(out(1, "SELFTEST failed bad\x1b[31m text\n", ""))).unwrap_err();
    assert!(!e.contains('\x1b'), "{e:?}");
    // nothing installed: nothing is launched
    let l = Fake { seen: Mutex::new(vec![]), answer: Ok(out(0, "SELFTEST ok x", "")) };
    let none = tmp("none");
    assert!(selftest(&l, Path::new("/root/fs"), &none, &none.join("s")).unwrap_err().contains("not in place"));
    assert!(l.seen.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_item_knows_installed_current_upgrade_and_broken_and_verify_runs_the_test_again() {
    use iyakki_shared::datasci::{Kind, Pin};
    let d = tmp("status");
    let pins = [Pin { name: "numpy", version: "2.5.4", kind: Kind::Wheel, url: "https://files.pythonhosted.org/x", sha256: "aa", mb: 1, licence: "x" }];
    assert_eq!(status(&d, &pins), Status::NotInstalled);
    toolkit(&d, &["numpy 2.5.4 aa"]);
    assert!(matches!(status(&d, &pins), Status::Current { .. }));
    // a newer table (another version or hash, or another file) offers the upgrade
    let newer = [Pin { version: "2.6.0", ..pins[0] }];
    assert_eq!(status(&d, &newer), Status::UpgradeAvailable { have: 1, wanted: 1 });
    let more = [pins[0], Pin { name: "scipy", ..pins[0] }];
    assert_eq!(status(&d, &more), Status::UpgradeAvailable { have: 1, wanted: 2 });
    // a record that does not match its folders is broken, and verify says so without launching anything
    std::fs::write(datasci::dir(&d).join("state.json"), "{not json").unwrap();
    assert!(matches!(status(&d, &pins), Status::Broken(_)));
    let l = Fake { seen: Mutex::new(vec![]), answer: Ok(out(0, "SELFTEST ok fine", "")) };
    assert!(verify(&l, Path::new("/root/fs"), &d, &d.join("scratch"), &pins).unwrap_err().contains("record"));
    assert!(l.seen.lock().unwrap().is_empty());
    toolkit(&d, &["numpy 2.5.4 aa"]);
    assert_eq!(verify(&l, Path::new("/root/fs"), &d, &d.join("scratch"), &pins).unwrap(), "fine");
    assert_eq!(l.seen.lock().unwrap().len(), 1);
    assert!(verify(&l, Path::new("/root/fs"), &tmp("nope"), &d.join("scratch"), &pins).unwrap_err().contains("not installed"));
    let _ = std::fs::remove_dir_all(&d);
}

/// A launcher that behaves like a script: it writes files into the writable folder the arguments name.
struct Writes(Vec<(&'static str, Vec<u8>)>, NsOutput, Mutex<Vec<Duration>>);

impl NsLauncher for Writes {
    fn launch(&self, args: &[String], timeout: Duration, _cap: usize) -> Result<NsOutput, String> {
        self.2.lock().unwrap().push(timeout);
        let rw = args.windows(3).find(|w| w[0] == "--bind-rw").map(|w| w[1].clone()).unwrap();
        for (n, b) in &self.0 {
            std::fs::write(Path::new(&rw).join(n), b).unwrap();
        }
        Ok(self.1.clone())
    }
}

#[test]
fn a_script_runs_in_the_sandbox_with_the_persons_time_limit_and_what_it_made_is_kept_and_the_work_folder_emptied() {
    let d = tmp("script");
    toolkit(&d, &["numpy 2.0 aa"]);
    let (scratch, keep) = (d.join("scratch"), d.join("keep"));
    let l = Writes(vec![("table.csv", b"a,b\n1,2\n".to_vec()), ("plot.png", vec![1, 2, 3]), (".hidden", vec![1])], out(0, "mean 3.5\x1b[31m\n", ""), Mutex::new(Vec::new()));
    let r = run_script(&l, Path::new("/root"), &d, &scratch, &keep, "print('mean 3.5')", 90).unwrap();
    assert_eq!(r.stdout, "mean 3.5[31m\n", "control characters other than new lines are gone");
    assert_eq!(r.files, ["plot.png (3 bytes)", "table.csv (8 bytes)"], "sorted, the script and hidden files left out");
    assert!(keep.join("table.csv").is_file() && !keep.join(".hidden").exists());
    assert_eq!(*l.2.lock().unwrap(), [Duration::from_secs(90)]);
    assert!(!scratch.join("run").exists(), "the work folder is emptied");
    // a failing script says where it stopped; a silent timeout says the limit; nothing runs for a bad request
    let bad = Writes(vec![], out(1, "", "Traceback (most recent call last):\n  File \"x\"\nZeroDivisionError: division by zero\n"), Mutex::new(Vec::new()));
    let e = run_script(&bad, Path::new("/root"), &d, &scratch, &keep, "1/0", 60).unwrap_err();
    assert!(e.contains("exit 1") && e.contains("ZeroDivisionError"), "{e}");
    let slow = Writes(vec![], NsOutput { code: None, signal: Some(9), timed_out: true, stdout: vec![], stderr: vec![] }, Mutex::new(Vec::new()));
    assert!(run_script(&slow, Path::new("/root"), &d, &scratch, &keep, "while True: pass", 5).unwrap_err().contains("within 5 seconds"));
    let never = Writes(vec![], out(0, "", ""), Mutex::new(Vec::new()));
    for (s, secs, why) in [("", 10, "empty"), ("x", 0, "1 to 300"), ("x", 301, "1 to 300")] {
        assert!(run_script(&never, Path::new("/root"), &d, &scratch, &keep, s, secs).unwrap_err().contains(why));
    }
    assert!(run_script(&never, Path::new("/root"), &d, &scratch, &keep, &"x".repeat(SCRIPT_MAX + 1), 10).unwrap_err().contains("longer than"));
    assert!(never.2.lock().unwrap().is_empty(), "a refused request never reached the sandbox");
    // no toolkit, no run
    let none = tmp("script-none");
    assert!(run_script(&never, Path::new("/root"), &none, &scratch, &keep, "print(1)", 10).unwrap_err().contains("not installed"));
}

/// A launcher that behaves like a script with inputs: it reads what was copied to `in/` and writes `out/`.
struct IoScript(Mutex<Vec<String>>);

impl NsLauncher for IoScript {
    fn launch(&self, args: &[String], _t: Duration, _c: usize) -> Result<NsOutput, String> {
        let rw = PathBuf::from(args.windows(3).find(|w| w[0] == "--bind-rw").map(|w| w[1].clone()).unwrap());
        let mut seen = self.0.lock().unwrap();
        seen.push(std::fs::read_to_string(rw.join("in/t.csv")).unwrap_or_default());
        seen.push(std::fs::read_dir(rw.join("in/cam")).map(|d| d.count().to_string()).unwrap_or_default());
        seen.push(format!("env:{}", args.join(" ").contains("IYAKKI_FRAME_CAP=3")));
        std::fs::write(rw.join("out/set.csv"), "f,l\n1,0\n").unwrap();
        std::fs::write(rw.join("out/extra.txt"), "ignored").unwrap();
        Ok(out(0, "ok", ""))
    }
}

struct Registered(Mutex<Vec<String>>);

impl crate::datasci_io::Sink for Registered {
    fn table(&self, n: &str, _: &Path, r: usize, c: usize) -> Result<String, String> {
        self.0.lock().unwrap().push(format!("{n} {r}x{c}"));
        Ok("Added to Data.".into())
    }
    fn image(&self, _: &str, _: &Path) -> Result<String, String> {
        Err("no".into())
    }
}

#[test]
fn a_run_with_inputs_and_named_outputs_copies_in_collects_only_those_and_removes_the_read_only_copies_afterwards() {
    use crate::datasci_io::{Input, Kind, RunIo, Source, Want};
    let d = tmp("io");
    toolkit(&d, &["numpy 2.0 aa"]);
    let src = d.join("src");
    std::fs::create_dir_all(src.join("pics")).unwrap();
    std::fs::write(src.join("t.csv"), "a\n1\n").unwrap();
    for n in ["1.jpg", "2.jpg", "3.jpg", "4.jpg"] {
        std::fs::write(src.join("pics").join(n), n).unwrap();
    }
    let io = RunIo {
        inputs: vec![Input { name: "t.csv".into(), source: Source::File(src.join("t.csv")) }, Input { name: "cam".into(), source: Source::Frames(src.join("pics")) }],
        outputs: vec![Want { file: "set.csv".into(), kind: Kind::Table }],
        frame_cap: Some(3),
    };
    let (scratch, keep) = (d.join("scratch"), d.join("keep"));
    let (l, sink) = (IoScript(Mutex::new(vec![])), Registered(Mutex::new(vec![])));
    let r = run_script_io(&l, Path::new("/root"), &d, &scratch, &keep, "print(1)", 30, &io, &sink).unwrap();
    assert_eq!(*l.0.lock().unwrap(), ["a\n1\n", "3", "env:true"], "the copies were there, capped at 3 frames, and the script was told the cap");
    assert_eq!(*sink.0.lock().unwrap(), ["set.csv 1x2"]);
    assert!(r.files.iter().any(|f| f.contains("in/t.csv")) && r.files.iter().any(|f| f.contains("set.csv: table, 1 rows, 2 columns. Added to Data.")), "{:?}", r.files);
    assert!(keep.join("set.csv").is_file() && !keep.join("extra.txt").exists(), "only the listed output is kept");
    assert!(!scratch.join("run").exists(), "the work folder with its read-only copies is gone");
    // a bad input stops before the sandbox
    let never = Fake { seen: Mutex::new(vec![]), answer: Ok(out(0, "", "")) };
    let bad = RunIo { inputs: vec![Input { name: "../x".into(), source: Source::File(src.join("t.csv")) }], ..Default::default() };
    assert!(run_script_io(&never, Path::new("/root"), &d, &scratch, &keep, "x", 10, &bad, &sink).is_err());
    assert!(never.seen.lock().unwrap().is_empty());
    assert!(!scratch.join("run").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_real_sink_never_overwrites_and_free_names_count_up() {
    let d = tmp("sink");
    assert_eq!(free_name(&d, "a.csv"), d.join("a.csv"));
    std::fs::write(d.join("a.csv"), "x").unwrap();
    assert_eq!(free_name(&d, "a.csv"), d.join("a-2.csv"));
    std::fs::write(d.join("a-2.csv"), "x").unwrap();
    assert_eq!(free_name(&d, "a.csv"), d.join("a-3.csv"));
    let home = d.join("home");
    let sink = DataSink { user: "ada".into(), home: home.clone() };
    let png = d.join("p.png");
    std::fs::write(&png, [0x89, b'P']).unwrap();
    assert!(crate::datasci_io::Sink::image(&sink, "p.png", &png).unwrap().contains("Pictures/ai-output/p.png"));
    assert!(crate::datasci_io::Sink::image(&sink, "p.png", &png).unwrap().contains("p-2.png"));
    assert!(home.join("Pictures/ai-output/p.png").is_file() && home.join("Pictures/ai-output/p-2.png").is_file());
    let csv = d.join("t.csv");
    std::fs::write(&csv, "a,b\n1,2\n3,4\n").unwrap();
    let said = crate::datasci_io::Sink::table(&sink, "t.csv", &csv, 2, 2).unwrap();
    assert!(said.contains("2 rows, 2 columns") && home.join("Data/ai-output/t.csv").is_file(), "{said}");
    let _ = std::fs::remove_dir_all(&d);
}
