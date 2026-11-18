// SPDX-License-Identifier: Apache-2.0
use super::*;
use std::sync::Mutex;

struct None_;
impl Tools for None_ {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        Err(format!("unknown {name}"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.into()
    }
}

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    refuse: bool,
}
impl Backend for Fake {
    fn preview(&self, team: &str, name: &str) -> Result<String, String> {
        if name == "missing" {
            return Err("this local model host is not switched on".into());
        }
        Ok(format!("kind: Service # {team}/{name}"))
    }
    fn publish(&self, team: &str, name: &str, environment: &str) -> Result<String, String> {
        self.calls.lock().unwrap().push(format!("{team}/{name}@{environment}"));
        if self.refuse {
            return Err("the Service was not applied: a second person with the role lead has to approve this deploy first".into());
        }
        Ok("service/gpu created".into())
    }
}

fn tools(f: &Arc<Fake>) -> TeamHostTools {
    TeamHostTools::new(Arc::new(None_), f.clone())
}

#[test]
fn the_tool_asks_every_time_and_the_card_shows_the_service() {
    let f = Arc::new(Fake::default());
    let t = tools(&f);
    let d = t.defs();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].spec.name, TOOL);
    assert_eq!(d[0].risk, Risk::Ask);
    let a = json!({"team": "red", "name": "gpu"});
    let card = t.preview(TOOL, &a).unwrap();
    assert!(card.contains("kind: Service # red/gpu") && card.contains("environment dev"), "{card}");
    assert!(t.grant_blocker(TOOL, &a).is_some());
    assert!(!t.pre_approved(TOOL) && !t.approved_always(TOOL));
    assert!(t.refusal(TOOL, &a).is_none());
    assert!(f.calls.lock().unwrap().is_empty(), "showing the card applies nothing");
}

#[test]
fn a_call_goes_to_the_backend_with_the_environment_and_a_refusal_comes_back_plain() {
    let f = Arc::new(Fake::default());
    let t = tools(&f);
    assert_eq!(t.call(TOOL, &json!({"team": "red", "name": "gpu", "environment": "preprod"})).unwrap(), "service/gpu created");
    assert_eq!(f.calls.lock().unwrap().as_slice(), ["red/gpu@preprod"]);
    let g = Arc::new(Fake { refuse: true, ..Default::default() });
    let e = tools(&g).call(TOOL, &json!({"team": "red", "name": "gpu"})).unwrap_err();
    assert!(e.contains("second person"), "{e}");
}

#[test]
fn bad_names_are_refused_before_anything_is_asked() {
    let f = Arc::new(Fake::default());
    let t = tools(&f);
    for a in [json!({}), json!({"team": "red"}), json!({"team": "a b", "name": "x"}), json!({"team": "red", "name": "x/y"}), json!({"team": "red", "name": "x", "environment": "d;e"})] {
        assert!(t.refusal(TOOL, &a).is_some(), "{a}");
        assert!(t.call(TOOL, &a).is_err(), "{a}");
    }
    assert!(t.refusal(TOOL, &json!({"team": "red", "name": "missing"})).unwrap().contains("not switched on"));
    assert!(f.calls.lock().unwrap().is_empty());
    assert!(t.call("other", &json!({})).unwrap_err().contains("unknown"));
}
