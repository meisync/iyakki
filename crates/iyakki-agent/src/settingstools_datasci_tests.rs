use super::*;
use crate::agent::Tools;
use crate::settingstools_tests::{assert_policy, rig, tools};
use std::sync::Mutex;

struct Fake(Mutex<Vec<String>>);

impl Backend for Fake {
    fn state(&self, _: &Path) -> String {
        "Installed and up to date.".into()
    }
    fn verify(&self, _: &Path, user: &str) -> Result<String, String> {
        self.0.lock().unwrap().push(format!("verify {user}"));
        Ok("scikit-learn fits a small model".into())
    }
    fn run(&self, _: &Path, user: &str, script: &str, secs: u64) -> Result<RunReport, String> {
        self.0.lock().unwrap().push(format!("run {user} {secs} {}", script.len()));
        Ok(RunReport { stdout: "42\n".into(), files: vec!["out.csv (3 bytes)".into()], keep: Some("/keep/1".into()) })
    }
}

#[test]
fn the_tools_are_free_to_read_ask_to_run_show_the_whole_script_and_check_the_words_before_the_sandbox() {
    let r = rig("datasci", true);
    let fake = Arc::new(Fake(Mutex::new(Vec::new())));
    let t = tools(&r, entries_with(&r.env, fake.clone()));
    assert_policy(&t, &["datasci_status"], &["datasci_verify", "python_run"]);
    assert_eq!(t.call("datasci_status", &json!({})).unwrap(), "Installed and up to date.");
    assert!(t.call("datasci_verify", &json!({})).unwrap().starts_with("The toolkit self-test passed"));
    let out = t.call("python_run", &json!({"script": "print(42)", "seconds": 20})).unwrap();
    assert!(out.contains("Output:\n42") && out.contains("out.csv (3 bytes)") && out.contains("/keep/1"), "{out}");
    assert_eq!(*fake.0.lock().unwrap(), ["verify ada", "run ada 20 9"]);
    // bad words never reach the sandbox
    for bad in [json!({"script": ""}), json!({"script": "x", "seconds": 0}), json!({"script": "x", "seconds": 301}), json!({"script": "x", "seconds": "ten"}), json!({})] {
        assert!(t.call("python_run", &bad).is_err(), "{bad}");
    }
    assert_eq!(fake.0.lock().unwrap().len(), 2);
    assert!(r.audits.lock().unwrap().iter().any(|l| l.starts_with("datasci.python_run")) && r.audits.lock().unwrap().iter().any(|l| l.starts_with("datasci.verify")));
    // the card carries the whole script
    let card = t.preview("python_run", &json!({"script": "import os\nprint(1)"})).unwrap_or_default();
    assert!(card.contains("import os\nprint(1)"), "{card}");
    // verification is for administrators; a standard user still reads the status and can run a script
    let u = rig("datasci-user", false);
    let tu = tools(&u, entries_with(&u.env, fake));
    let names: Vec<String> = tu.defs().into_iter().map(|d| d.spec.name).collect();
    assert!(names.contains(&"datasci_status".to_string()) && names.contains(&"python_run".to_string()) && !names.contains(&"datasci_verify".to_string()), "{names:?}");
}

struct IoFake(Mutex<Vec<RunIo>>);

impl Backend for IoFake {
    fn state(&self, _: &Path) -> String {
        String::new()
    }
    fn verify(&self, _: &Path, _: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn run(&self, _: &Path, _: &str, _: &str, _: u64) -> Result<RunReport, String> {
        Ok(RunReport { stdout: String::new(), files: vec![], keep: None })
    }
    fn run_io(&self, _: &Path, _: &str, _: &str, _: u64, io: &RunIo) -> Result<RunReport, String> {
        self.0.lock().unwrap().push(io.clone());
        Ok(RunReport { stdout: "done\n".into(), files: vec!["set.csv: table, 2 rows, 2 columns. Added to Data.".into()], keep: None })
    }
}

#[test]
fn python_run_takes_inputs_from_the_persons_home_a_frame_cap_and_named_outputs_and_the_card_says_so() {
    let r = rig("datasci-io", false);
    let home = crate::scope::home_root_for_writes("ada", &r.dir);
    std::fs::create_dir_all(home.join("Pictures/cam")).unwrap();
    std::fs::write(home.join("table.csv"), "a,b\n1,2\n").unwrap();
    let fake = Arc::new(IoFake(Mutex::new(vec![])));
    let t = tools(&r, entries_with(&r.env, fake.clone()));
    let args = json!({
        "script": "print(1)",
        "inputs": [{"name": "t.csv", "path": "table.csv"}, {"name": "cam", "path": "Pictures/cam", "frames": true}],
        "outputs": [{"file": "set.csv", "kind": "table"}, {"file": "plot.png", "kind": "image"}],
        "frame_cap": 120
    });
    let card = t.preview("python_run", &args).unwrap();
    assert!(card.contains("read-only copies of t.csv, cam (frames)") && card.contains("set.csv, plot.png") && card.contains("Frame cap 120") && card.contains("print(1)"), "{card}");
    let said = t.call("python_run", &args).unwrap();
    assert!(said.contains("done") && said.contains("Added to Data"), "{said}");
    let seen = fake.0.lock().unwrap();
    assert_eq!(seen[0].frame_cap, Some(120));
    assert_eq!(seen[0].inputs.len(), 2);
    assert!(matches!(&seen[0].inputs[1].source, Source::Frames(p) if p.ends_with("Pictures/cam")) && matches!(&seen[0].inputs[0].source, Source::File(p) if p.ends_with("table.csv")));
    assert_eq!(seen[0].outputs[0].kind, Kind::Table);
    drop(seen);
    assert!(r.audits.lock().unwrap().iter().any(|l| l.starts_with("datasci.python_run") && l.contains("2 input(s), 2 output(s)") && !l.contains("table.csv")));
    // refusals before the sandbox: outside the home, links out, bad names and kinds, a frame cap off the scale, both path and handle
    let n = |extra: serde_json::Value| { let mut a = json!({"script": "x"}); for (k, v) in extra.as_object().unwrap() { a[k] = v.clone(); } t.call("python_run", &a) };
    assert!(n(json!({"inputs": [{"name": "p", "path": "/tmp"}]})).unwrap_err().contains("outside your home"));
    assert!(n(json!({"inputs": [{"name": "p", "path": "../../etc/hostname"}]})).is_err());
    assert!(n(json!({"inputs": [{"name": "p"}]})).unwrap_err().contains("path"));
    assert!(n(json!({"inputs": [{"name": "p", "path": "table.csv", "handle": "h1"}]})).is_err());
    assert!(n(json!({"inputs": [{"name": "../p", "path": "table.csv"}]})).is_err());
    assert!(n(json!({"inputs": [{"name": "p", "handle": "nope"}]})).unwrap_err().contains("no Data table"));
    assert!(n(json!({"outputs": [{"file": "a/b.csv", "kind": "table"}]})).is_err());
    assert!(n(json!({"outputs": [{"file": "a.csv", "kind": "movie"}]})).unwrap_err().contains("table, image or file"));
    assert!(n(json!({"frame_cap": 0})).is_err() && n(json!({"frame_cap": 18001})).is_err());
    assert_eq!(fake.0.lock().unwrap().len(), 1, "a refused request never reached the sandbox");
    // the default backend that cannot do inputs says so rather than ignoring them
    let plain = Arc::new(Fake(Mutex::new(vec![])));
    let t2 = tools(&r, entries_with(&r.env, plain));
    assert!(t2.call("python_run", &json!({"script": "x", "outputs": [{"file": "a.csv", "kind": "table"}]})).unwrap_err().contains("cannot run a script with inputs"));
}
