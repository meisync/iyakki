//! Every method of `agent::Tools` reaches the tools at the bottom of a stack of wrappers that decide nothing themselves. A recording tool
//! implements all of them; three plain wrappers (each only has `inner_tools`) stand over it. If a method is added to the trait without
//! its forwarding, or the forwarding is broken, this fails. (The source scan `tests/tools_wrappers_forward.rs` makes sure every real
//! wrapper in the repository has `inner_tools`.)

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<String>>,
}

impl Recorder {
    fn rec(&self, what: &str) {
        self.seen.lock().unwrap().push(what.to_string());
    }
}

impl Tools for Recorder {
    fn defs(&self) -> Vec<ToolDef> {
        self.rec("defs");
        vec![ToolDef { spec: ToolSpec { name: "bottom_tool".into(), description: "d".into(), schema: json!({"type": "object"}) }, risk: Risk::Ask, untrusted: false }]
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        self.rec("call");
        Ok(format!("called {name}"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        self.rec("describe");
        format!("describe {name}")
    }
    fn preview(&self, _: &str, _: &Value) -> Option<String> {
        self.rec("preview");
        Some("the card".into())
    }
    fn refusal(&self, _: &str, _: &Value) -> Option<String> {
        self.rec("refusal");
        Some("refused".into())
    }
    fn pre_approved(&self, _: &str) -> bool {
        self.rec("pre_approved");
        true
    }
    fn approved_always(&self, _: &str) -> bool {
        self.rec("approved_always");
        false
    }
    fn grant_blocker(&self, _: &str, _: &Value) -> Option<String> {
        self.rec("grant_blocker");
        Some("never granted".into())
    }
    fn grant_scope(&self, _: &str, _: &Value) -> String {
        self.rec("grant_scope");
        "the scope".into()
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.rec("unresolved_work");
        vec!["a row".into()]
    }
    fn correct_claims(&self, _: &str) -> Option<String> {
        self.rec("correct_claims");
        Some("corrected".into())
    }
    fn begin_step(&self) {
        self.rec("begin_step");
    }
    fn should_end_run(&self) -> Option<String> {
        self.rec("should_end_run");
        Some("stop".into())
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.rec("take_diagnosis");
        Some(("r1".into(), "why".into()))
    }
    fn note_external(&self, _: &str, _: &Value, _: bool) {
        self.rec("note_external");
    }
    fn take_notes(&self) -> Vec<String> {
        self.rec("take_notes");
        vec!["a note".into()]
    }
    fn jobs_running(&self) -> Vec<String> {
        self.rec("jobs_running");
        vec!["a job".into()]
    }
}

/// A wrapper that adds a tool of its own and implements nothing else: the shape every real wrapper should have.
struct Plain {
    inner: Arc<dyn Tools>,
    own: &'static str,
}

impl Tools for Plain {
    fn inner_tools(&self) -> Option<&dyn Tools> {
        Some(&*self.inner)
    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.push(ToolDef { spec: ToolSpec { name: self.own.into(), description: "d".into(), schema: json!({"type": "object"}) }, risk: Risk::Safe, untrusted: false });
        v
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        self.inner.call(name, args)
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        self.inner.describe(name, args)
    }
}

/// Every method of the trait, in the order of its declaration: when one is added, add it here too (the count below fails until it is).
const METHODS: [&str; 17] = ["defs", "call", "describe", "preview", "refusal", "pre_approved", "approved_always", "grant_blocker", "grant_scope", "unresolved_work", "correct_claims", "begin_step", "should_end_run", "take_diagnosis", "note_external", "take_notes", "jobs_running"];

#[test]
fn every_method_of_the_tools_underneath_is_reachable_through_a_stack_of_wrappers() {
    let bottom = Arc::new(Recorder::default());
    let top = Plain { inner: Arc::new(Plain { inner: Arc::new(Plain { inner: bottom.clone(), own: "a" }), own: "b" }), own: "c" };
    let a = json!({});
    assert_eq!(top.defs().len(), 4, "the bottom's tool and one of each wrapper");
    assert_eq!(top.call("t", &a).unwrap(), "called t");
    assert_eq!(top.describe("t", &a), "describe t");
    assert_eq!(top.preview("t", &a).as_deref(), Some("the card"));
    assert_eq!(top.refusal("t", &a).as_deref(), Some("refused"));
    assert!(top.pre_approved("t"));
    assert!(!top.approved_always("t"), "the bottom said no: a wrapper must not turn it into the default yes");
    assert_eq!(top.grant_blocker("t", &a).as_deref(), Some("never granted"));
    assert_eq!(top.grant_scope("t", &a), "the scope");
    assert_eq!(top.unresolved_work(), ["a row"]);
    assert_eq!(top.correct_claims("done").as_deref(), Some("corrected"));
    top.begin_step();
    assert_eq!(top.should_end_run().as_deref(), Some("stop"));
    assert_eq!(top.take_diagnosis(), Some(("r1".to_string(), "why".to_string())));
    top.note_external("Write", &a, true);
    assert_eq!(top.take_notes(), ["a note"]);
    assert_eq!(top.jobs_running(), ["a job"]);
    let seen = bottom.seen.lock().unwrap().clone();
    for m in METHODS {
        assert!(seen.iter().any(|s| s == m), "`{m}` did not reach the tools at the bottom: {seen:?}");
    }
    assert_eq!(METHODS.len(), 17);
}

#[test]
fn a_set_of_tools_that_wraps_nothing_keeps_the_old_defaults() {
    struct Alone;
    impl Tools for Alone {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, _: &str, _: &Value) -> Result<String, String> {
            Err("none".into())
        }
        fn describe(&self, name: &str, _: &Value) -> String {
            name.into()
        }
    }
    let t = Alone;
    let a = json!({});
    assert!(t.preview("x", &a).is_none() && t.refusal("x", &a).is_none() && !t.pre_approved("x"));
    assert!(t.approved_always("x"), "the default of a set that wraps nothing is still yes");
    assert_eq!(t.grant_blocker("vault_request", &a).is_some(), crate::agent::hard_ask("vault_request", &a).is_some());
    assert!(t.unresolved_work().is_empty() && t.take_notes().is_empty() && t.jobs_running().is_empty());
    assert!(t.correct_claims("x").is_none() && t.should_end_run().is_none() && t.take_diagnosis().is_none());
}
