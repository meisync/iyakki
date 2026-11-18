use super::*;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;

static N: AtomicU32 = AtomicU32::new(0);

/// A fake sysfs tree and a data folder, both temp folders of this test.
fn rig() -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("iyakki-fansvc-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = fs::remove_dir_all(&root);
    let sys = root.join("sys");
    let dir = root.join("data");
    let smc = sys.join("class/hwmon/hwmon2");
    fs::create_dir_all(&smc).unwrap();
    fs::create_dir_all(&dir).unwrap();
    fs::write(smc.join("name"), "applesmc\n").unwrap();
    for (k, v) in [("fan1_min", "1200"), ("fan1_max", "6200"), ("fan1_manual", "0"), ("fan1_output", "1200"), ("fan1_input", "1210")] {
        fs::write(smc.join(k), v).unwrap();
    }
    let ct = sys.join("class/hwmon/hwmon3");
    fs::create_dir_all(&ct).unwrap();
    fs::write(ct.join("name"), "coretemp\n").unwrap();
    fs::write(ct.join("temp1_input"), "80000").unwrap();
    (sys, dir)
}

fn file(sys: &Path, f: &str) -> String {
    fs::read_to_string(sys.join("class/hwmon/hwmon2").join(f)).unwrap().trim().to_string()
}

fn wait(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..300 {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn the_setting_is_off_by_default_and_a_bad_curve_is_never_saved() {
    let (_, dir) = rig();
    assert!(!load(&dir).enabled);
    assert!(save(&dir, &Setting { enabled: true, low: 90, high: 60, ..Setting::default() }).is_err());
    assert!(!dir.join(SETTING_FILE).exists());
    save(&dir, &Setting { enabled: true, ..Setting::default() }).unwrap();
    assert!(load(&dir).enabled);
    fs::write(dir.join(SETTING_FILE), "not json").unwrap();
    assert!(!load(&dir).enabled, "an unreadable file means off");
}

#[test]
fn nothing_is_written_while_the_setting_is_off_then_it_takes_over_follows_the_curve_and_gives_the_fans_back() {
    let (sys, dir) = rig();
    let stop = Arc::new(AtomicBool::new(false));
    let events = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let h = {
        let (sys, dir, stop, events) = (sys.clone(), dir.clone(), stop.clone(), events.clone());
        std::thread::spawn(move || run(&sys, &dir, &stop, Duration::from_millis(20), &mut |e, d| events.lock().unwrap().push(format!("{e}: {d}"))))
    };
    assert!(wait(|| status(&dir).contains("off")));
    assert_eq!((file(&sys, "fan1_manual"), file(&sys, "fan1_output")), ("0".into(), "1200".into()), "off writes nothing");
    save(&dir, &Setting { enabled: true, ..Setting::default() }).unwrap();
    assert!(wait(|| file(&sys, "fan1_manual") == "1"));
    let hot: u32 = file(&sys, "fan1_output").parse().unwrap();
    assert!(hot > 1200 && hot <= 6200, "{hot}");
    assert!(status(&dir).contains("controlled by IYAKKI"));
    // a cooler machine lowers the speed once it is far enough below the last step
    fs::write(sys.join("class/hwmon/hwmon3/temp1_input"), "50000").unwrap();
    assert!(wait(|| file(&sys, "fan1_output") == "1200"));
    // turning it off hands the fans back
    save(&dir, &Setting { enabled: false, ..Setting::default() }).unwrap();
    assert!(wait(|| file(&sys, "fan1_manual") == "0"));
    // an implausible reading hands them back too, and the service keeps running
    save(&dir, &Setting { enabled: true, ..Setting::default() }).unwrap();
    assert!(wait(|| file(&sys, "fan1_manual") == "1"));
    fs::write(sys.join("class/hwmon/hwmon3/temp1_input"), "0").unwrap();
    assert!(wait(|| file(&sys, "fan1_manual") == "0"));
    fs::write(sys.join("class/hwmon/hwmon3/temp1_input"), "70000").unwrap();
    assert!(wait(|| file(&sys, "fan1_manual") == "1"));
    stop.store(true, Ordering::SeqCst);
    h.join().unwrap();
    assert_eq!(file(&sys, "fan1_manual"), "0", "stopping gives the fans back");
    let ev = events.lock().unwrap().join("\n");
    for want in ["fan.take-over", "fan.release", "implausible temperature", "the service stopped"] {
        assert!(ev.contains(want), "{want} in {ev}");
    }
    let _ = fs::remove_dir_all(sys.parent().unwrap());
}

#[test]
fn a_computer_without_fans_writes_nothing_and_says_so_once() {
    let (sys, dir) = rig();
    fs::remove_file(sys.join("class/hwmon/hwmon2/name")).unwrap();
    assert!(!supported(&sys));
    save(&dir, &Setting { enabled: true, ..Setting::default() }).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let events = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let h = {
        let (sys, dir, stop, events) = (sys.clone(), dir.clone(), stop.clone(), events.clone());
        std::thread::spawn(move || run(&sys, &dir, &stop, Duration::from_millis(10), &mut |e, _| events.lock().unwrap().push(e.to_string())))
    };
    assert!(wait(|| status(&dir).contains("no applesmc")));
    std::thread::sleep(Duration::from_millis(80));
    stop.store(true, Ordering::SeqCst);
    h.join().unwrap();
    assert_eq!(events.lock().unwrap().iter().filter(|e| *e == "fan.unsupported").count(), 1);
    assert_eq!(file_or(&sys), "0");
    let _ = fs::remove_dir_all(sys.parent().unwrap());
}

fn file_or(sys: &Path) -> String {
    fs::read_to_string(sys.join("class/hwmon/hwmon2/fan1_manual")).unwrap().trim().to_string()
}
