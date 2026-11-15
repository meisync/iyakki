use std::sync::Mutex;

use super::*;
use crate::wizardrun::Env;

struct Inner;
impl Tools for Inner {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        Err(format!("inner got {name}"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.into()
    }
}

struct FakeEnv(Mutex<Vec<String>>);
impl Env for FakeEnv {
    fn check(&self, check: &str, param: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(format!("check {check} {param}"));
        Ok(())
    }
    fn migrate(&self, r: &str, _: Option<&str>, _: Option<&str>, a: &BTreeMap<String, Value>) -> Result<String, String> {
        self.0.lock().unwrap().push(format!("migrate {r} keys={}", a.keys().cloned().collect::<Vec<_>>().join(",")));
        Ok("moved".into())
    }
}

struct Fake {
    specs: Vec<WizardSpec>,
    pending: Mutex<Vec<(String, String, String, String)>>,
    run: Arc<Mutex<Option<(String, Run)>>>,
    env: Arc<FakeEnv>,
    audit: Mutex<Vec<String>>,
}
impl Backend for Fake {
    fn installed(&self) -> Vec<(String, String, WizardSpec)> {
        self.specs.iter().map(|s| ("notes".to_string(), "1.0.0".to_string(), s.clone())).collect()
    }
    fn pending(&self) -> Vec<(String, String, String, String)> {
        self.pending.lock().unwrap().clone()
    }
    fn clear_pending(&self, app: &str, wizard: &str) {
        self.pending.lock().unwrap().retain(|p| !(p.0 == app && p.1 == wizard));
    }
    fn env(&self) -> Arc<dyn Env> {
        self.env.clone()
    }
    fn run(&self) -> Arc<Mutex<Option<(String, Run)>>> {
        self.run.clone()
    }
    fn audit(&self, what: &str, detail: &str) {
        self.audit.lock().unwrap().push(format!("{what} {detail}"));
    }
}

fn rig() -> (WizardTools, Arc<Fake>) {
    let spec: WizardSpec = serde_json::from_value(json!({
        "id": "setup", "title": "Setup", "trigger": {"on": "on_install"},
        "steps": [
            {"kind": "form", "id": "who", "title": "Who", "fields": [
                {"id": "name", "label": "Name", "kind": "text", "required": true, "rules": [{"rule": "min_len", "value": 3}]},
                {"id": "token", "label": "Token", "kind": "secret"}]},
            {"kind": "check", "id": "net", "title": "Server reachable", "check": "net.reach", "param": "example.com:443"},
            {"kind": "migration", "id": "mig", "title": "Move data", "migration": "m1"},
            {"kind": "summary", "id": "done", "title": "Ready", "text": "All set."}]
    }))
    .unwrap();
    let be = Arc::new(Fake { specs: vec![spec], pending: Mutex::new(vec![("notes".into(), "setup".into(), "on_install".into(), "Setup".into())]), run: Arc::new(Mutex::new(None)), env: Arc::new(FakeEnv(Mutex::new(vec![]))), audit: Mutex::new(vec![]) });
    (WizardTools::new(Arc::new(Inner), be.clone()), be)
}

#[test]
fn the_tools_exist_only_the_run_asks_and_everything_else_is_free() {
    let (t, _) = rig();
    let defs = t.defs();
    assert_eq!(defs.len(), 5);
    for d in &defs {
        assert!(matches!(d.risk, Risk::Ask) == (d.spec.name == CONTINUE), "{}", d.spec.name);
    }
    assert!(t.grant_blocker(CONTINUE, &json!({})).is_some() && !t.approved_always(CONTINUE) && !t.pre_approved(CONTINUE));
    assert_eq!(t.call("other", &json!({})).unwrap_err(), "inner got other");
}

#[test]
fn a_wizard_is_listed_started_answered_run_after_a_card_and_finished() {
    let (t, be) = rig();
    let p = t.call(PENDING, &json!({})).unwrap();
    assert!(p.contains("notes / setup (on_install): Setup") && p.contains("notes 1.0.0 / setup"), "{p}");
    assert!(t.refusal(CONTINUE, &json!({})).unwrap().contains("wizard_start"));
    let s = t.call(START, &json!({"app": "notes", "wizard": "setup"})).unwrap();
    assert!(s.contains("name [text] Name (needed)") && s.contains("token [secret]") && s.contains("the person enters it in the app"), "{s}");
    assert!(be.pending().is_empty(), "starting takes it off the waiting list");
    assert!(t.call(START, &json!({"app": "notes", "wizard": "setup"})).unwrap_err().contains("already running"));
    // a wrong answer changes nothing and says which field
    assert_eq!(t.call(ANSWER, &json!({"values": {"name": "ab"}})).unwrap_err(), "name: at least 3 characters");
    let a = t.call(ANSWER, &json!({"values": {"name": "amy", "token": "s3cret"}})).unwrap();
    assert!(a.contains("Next: Server reachable") && !a.contains("s3cret"), "{a}");
    // the card lists what will run, then the steps run
    let card = t.preview(CONTINUE, &json!({})).unwrap();
    assert!(card.contains("check net.reach example.com:443") && card.contains("migration m1"), "{card}");
    assert!(be.env.0.lock().unwrap().is_empty(), "nothing ran before the person approved");
    let r = t.call(CONTINUE, &json!({})).unwrap();
    assert!(r.contains("All set.") && r.contains("net: net.reach passed"), "{r}");
    assert_eq!(*be.env.0.lock().unwrap(), vec!["check net.reach example.com:443".to_string(), "migrate m1 keys=name".to_string()], "the secret never reached the migration");
    assert!(t.call(CONTINUE, &json!({})).unwrap().contains("finished"));
    let audit = be.audit.lock().unwrap().join("\n");
    assert!(audit.contains("wizard.start notes/setup") && audit.contains("wizard.finish") && !audit.contains("s3cret"), "{audit}");
}

#[test]
fn cancel_stops_it_and_a_missing_wizard_or_run_is_said_plainly() {
    let (t, _) = rig();
    assert!(t.call(START, &json!({"app": "notes", "wizard": "nope"})).unwrap_err().contains("no such wizard"));
    assert!(t.call(ANSWER, &json!({"values": {}})).unwrap_err().contains("wizard_start"));
    t.call(START, &json!({"app": "notes", "wizard": "setup"})).unwrap();
    assert_eq!(t.call(CANCEL, &json!({})).unwrap(), "The wizard was cancelled.");
    // a cancelled run does not block the next one
    assert!(t.call(START, &json!({"app": "notes", "wizard": "setup"})).is_ok());
}

#[test]
fn the_real_environment_refuses_paths_outside_the_home_and_unknown_checks_and_has_no_migration_yet() {
    let dir = std::env::temp_dir().join(format!("iyakki-wizenv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    struct Dead;
    impl iyakki_proxy::Upstream for Dead {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }
    let svc = crate::services::Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let env = RealEnv { svc, user: "amy".into(), app: "notes".into() };
    assert!(env.check("path.readable", "/etc/hostname").unwrap_err().contains("outside your own folder"));
    assert!(env.check("path.readable", "../x").unwrap_err().contains("outside"));
    assert!(env.check("teleport", "x").unwrap_err().contains("does not know the check"));
    assert!(env.migrate("m1", None, None, &BTreeMap::new()).unwrap_err().contains("no migration called m1"));
    assert!(env.check("net.reach", "no-such-host.invalid").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_waiting_list_is_kept_per_person_without_doubles_and_goes_when_the_app_does() {
    use crate::store::wizardhook::*;
    let dir = std::env::temp_dir().join(format!("iyakki-wizhook-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("wizards")).unwrap();
    let p = |app: &str, w: &str| Pending { app: app.into(), wizard: w.into(), title: "T".into(), trigger: "on_install".into(), from_version: None, queued_at: 5 };
    let body = serde_json::json!({"schema": 1, "pending": [p("notes", "setup"), p("notes", "second"), p("mail", "setup")], "added_by_a_newer_release": true});
    std::fs::write(dir.join("wizards/amy.json"), body.to_string()).unwrap();
    assert_eq!(pending(&dir, "amy").len(), 3);
    assert!(pending(&dir, "bob").is_empty() && pending(&dir, "../x").is_empty(), "another person and a bad name see nothing");
    assert!(clear(&dir, "amy", "notes", "setup") && !clear(&dir, "amy", "notes", "setup"));
    forget_app(&dir, "amy", "mail");
    assert_eq!(pending(&dir, "amy").iter().map(|x| x.wizard.as_str()).collect::<Vec<_>>(), ["second"]);
    let kept: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("wizards/amy.json")).unwrap()).unwrap();
    assert_eq!(kept["added_by_a_newer_release"], true, "a field of a newer release is written back");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_outline_says_what_a_wizard_asks_checks_and_moves_before_anything_runs() {
    let (_, be) = rig();
    let spec = be.installed()[0].2.clone();
    assert_eq!(crate::store::wizardhook::outline(&spec), "asks 2 questions · checks: net.reach example.com:443 · moves data: m1");
    let bare: WizardSpec = serde_json::from_value(json!({"id": "x", "trigger": {"on": "on_install"}, "steps": [{"kind": "summary", "id": "s", "text": "Hi"}]})).unwrap();
    assert_eq!(crate::store::wizardhook::outline(&bare), "has nothing to ask or run");
}
