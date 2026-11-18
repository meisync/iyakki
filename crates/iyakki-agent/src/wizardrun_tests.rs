use std::cell::RefCell;

use serde_json::json;

use super::*;

fn spec(v: Value) -> WizardSpec {
    serde_json::from_value(v).unwrap()
}

fn setup() -> WizardSpec {
    spec(json!({
        "id": "setup", "trigger": {"on": "on_install"},
        "steps": [
            {"kind": "form", "id": "who", "title": "Who", "fields": [
                {"id": "name", "label": "Name", "kind": "text", "required": true, "rules": [{"rule": "min_len", "value": 3}, {"rule": "no_whitespace"}]},
                {"id": "mode", "label": "Mode", "kind": "choice", "default": "fast", "choices": [{"value": "fast", "label": "Fast"}, {"value": "safe", "label": "Safe"}]},
                {"id": "token", "label": "Token", "kind": "secret"},
                {"id": "dir", "label": "Folder", "kind": "path", "default": "/data"},
                {"id": "on", "label": "On", "kind": "toggle", "default": true}
            ]},
            {"kind": "check", "id": "net", "title": "Server reachable", "check": "net.reach", "param": "example.com:443"},
            {"kind": "migration", "id": "mig", "title": "Move data", "migration": "m1", "to": "2.0.0"},
            {"kind": "summary", "id": "done", "title": "Ready", "text": "All set."}
        ]
    }))
}

struct Fake {
    net_ok: bool,
    calls: RefCell<Vec<String>>,
}
impl Env for Fake {
    fn check(&self, check: &str, param: &str) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("check {check} {param}"));
        if self.net_ok { Ok(()) } else { Err("could not connect".into()) }
    }
    fn migrate(&self, reference: &str, from: Option<&str>, to: Option<&str>, answers: &BTreeMap<String, Value>) -> Result<String, String> {
        self.calls.borrow_mut().push(format!("migrate {reference} {from:?} {to:?} keys={}", answers.keys().cloned().collect::<Vec<_>>().join(",")));
        Ok("moved 3 files".into())
    }
}

fn vals(v: Value) -> BTreeMap<String, Value> {
    v.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

#[test]
fn a_full_run_checks_answers_runs_the_automatic_steps_and_ends_at_the_summary() {
    let mut r = Run::start(setup(), "on_install", None).unwrap();
    let View::Form { fields, .. } = r.view() else { panic!("a form first") };
    assert_eq!(fields.iter().find(|f| f.id == "mode").unwrap().value, Some(json!("fast")), "defaults are shown");
    assert!(fields.iter().find(|f| f.id == "token").unwrap().value.is_none());
    r.submit(&vals(json!({"name": "amy", "token": "s3cret"}))).unwrap();
    assert!(matches!(r.view(), View::Working { .. }));
    let env = Fake { net_ok: true, calls: RefCell::new(vec![]) };
    r.advance(&env);
    assert_eq!(*env.calls.borrow(), vec!["check net.reach example.com:443".to_string(), "migrate m1 None Some(\"2.0.0\") keys=dir,mode,name,on".to_string()], "no secret reaches the migration");
    assert_eq!(r.view(), View::Summary { title: "Ready".into(), text: "All set.".into() });
    r.finish();
    assert_eq!(r.state(), &State::Done);
    assert!(!r.log().join("\n").contains("s3cret"));
    assert!(!r.answers_redacted().contains_key("token"));
    assert_eq!(r.take_answers().get("token"), Some(&json!("s3cret")));
}

#[test]
fn answers_are_checked_per_field_and_nothing_is_kept_when_one_is_wrong() {
    let mut r = Run::start(setup(), "on_install", None).unwrap();
    let e = r.submit(&vals(json!({"name": "a b", "mode": "other", "dir": "../x", "on": "yes"}))).unwrap_err();
    let by = |f: &str| e.iter().find(|x| x.field == f).map(|x| x.message.clone());
    assert_eq!(by("name").as_deref(), Some("no spaces"), "{e:?}");
    assert_eq!(r.submit(&vals(json!({"name": "ab"}))).unwrap_err()[0].message, "at least 3 characters");
    assert_eq!(by("mode").as_deref(), Some("pick one of the choices"));
    assert_eq!(by("dir").as_deref(), Some("an absolute path without .."));
    assert_eq!(by("on").as_deref(), Some("choose on or off"));
    assert!(r.answers_redacted().is_empty() && matches!(r.view(), View::Form { .. }));
    assert_eq!(r.submit(&vals(json!({"name": ""}))).unwrap_err()[0].message, "this is needed");
    assert_eq!(r.submit(&vals(json!({"nope": 1}))).unwrap_err()[0].message, "this step has no such field");
    r.submit(&vals(json!({"name": "amy"}))).unwrap();
}

#[test]
fn a_failed_check_ends_the_wizard_with_the_sentence_and_the_migration_never_runs() {
    let mut r = Run::start(setup(), "on_install", None).unwrap();
    r.submit(&vals(json!({"name": "amy"}))).unwrap();
    let env = Fake { net_ok: false, calls: RefCell::new(vec![]) };
    r.advance(&env);
    assert_eq!(r.state(), &State::Failed("Server reachable: could not connect".into()));
    assert_eq!(env.calls.borrow().len(), 1, "the migration was not tried");
    assert_eq!(r.view(), View::Finished(State::Failed("Server reachable: could not connect".into())));
    assert!(r.submit(&vals(json!({"name": "bob"}))).is_err());
}

#[test]
fn what_this_iyakki_cannot_run_is_refused_before_it_starts_and_the_trigger_must_match() {
    let mut unknown = serde_json::to_value(setup()).unwrap();
    unknown["steps"][1] = json!({"kind": "teleport", "id": "net"});
    let e = Run::start(spec(unknown), "on_install", None).err().unwrap();
    assert!(e.contains("does not know the step kind \"teleport\""), "{e}");
    let mut newer = serde_json::to_value(setup()).unwrap();
    newer["wizard_version"] = json!(9);
    assert!(Run::start(spec(newer), "on_install", None).err().unwrap().contains("Needs a newer IYAKKI"));
    assert!(Run::start(setup(), "on_remove", None).err().unwrap().contains("runs on"));
    assert!(Run::start(setup(), "sometimes", None).is_err());
    let upd = spec(json!({"id": "up", "trigger": {"on": "on_update", "from": "<2"}, "steps": [{"kind": "migration", "id": "m", "migration": "x"}]}));
    assert!(Run::start(upd.clone(), "on_update", Some("2.1.0")).err().unwrap().contains("not for the version"));
    assert!(Run::start(upd.clone(), "on_update", None).is_err());
    let mut r = Run::start(upd, "on_update", Some("1.4.0")).unwrap();
    let env = Fake { net_ok: true, calls: RefCell::new(vec![]) };
    r.advance(&env);
    assert_eq!(env.calls.borrow()[0], "migrate x Some(\"1.4.0\") None keys=", "the version being updated from reaches the migration");
    assert_eq!(r.state(), &State::Done);
}

#[test]
fn cancelling_stops_the_wizard_and_changes_nothing_after() {
    let mut r = Run::start(setup(), "on_install", None).unwrap();
    r.cancel();
    assert_eq!(r.state(), &State::Cancelled);
    let env = Fake { net_ok: true, calls: RefCell::new(vec![]) };
    r.advance(&env);
    r.finish();
    assert!(env.calls.borrow().is_empty() && r.state() == &State::Cancelled);
}
