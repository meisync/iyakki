use std::sync::Mutex;

use super::*;
use crate::k8s::FakeKube;

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

struct Fake {
    kube: Arc<FakeKube>,
    names: Vec<(&'static str, &'static str)>,
    audit: Mutex<Vec<String>>,
}

impl Backend for Fake {
    fn clusters(&self) -> Vec<Cluster1> {
        self.names.iter().map(|(id, name)| Cluster1 { id: id.to_string(), name: name.to_string(), target: Target::default(), kube: self.kube.clone() }).collect()
    }
    fn now(&self) -> u64 {
        2_000_000_000
    }
    fn audit(&self, a: &str, d: &str) {
        self.audit.lock().unwrap().push(format!("{a} {d}"));
    }
}

fn crd() -> Value {
    json!({"kind":"CustomResourceDefinition","metadata":{"name":"widgets.example.com","creationTimestamp":"2026-01-01T00:00:00Z"},
        "spec":{"group":"example.com","scope":"Namespaced","names":{"kind":"Widget","plural":"widgets"},"versions":[{"name":"v1","served":true,"storage":true}]}})
}

fn widget(name: &str, ns: &str, ready: bool) -> Value {
    json!({"kind":"Widget","metadata":{"name":name,"namespace":ns,"creationTimestamp":"2026-11-13T00:00:00Z"},"status":{"conditions":[{"type":"Ready","status": if ready { "True" } else { "False" },"reason":"Degraded"}]}})
}

fn rig() -> (K8sExpTools, Arc<Fake>) {
    let kube = Arc::new(FakeKube::new());
    kube.answer("get customresourcedefinitions -o json", &json!({"items":[crd()]}).to_string());
    kube.answer("get customresourcedefinitions widgets.example.com -o json", &crd().to_string());
    kube.answer("get widgets.example.com -A -o json", &json!({"items":[widget("alpha", "shop", true), widget("beta", "empty", false)]}).to_string());
    kube.answer("-n shop get widgets.example.com -o json", &json!({"items":[widget("alpha", "shop", true)]}).to_string());
    let f = Arc::new(Fake { kube, names: vec![("local", "Dev cluster"), ("lab", "Lab")], audit: Mutex::new(vec![]) });
    (K8sExpTools::new(Arc::new(Inner), f.clone()), f)
}

#[test]
fn the_tool_is_read_only_free_and_marked_untrusted() {
    let (t, _) = rig();
    let d = t.defs().into_iter().find(|d| d.spec.name == TOOL).unwrap();
    assert_eq!(d.risk, Risk::Safe);
    assert!(d.untrusted, "what a cluster prints is data, never an instruction");
    assert!(t.pre_approved(TOOL));
    assert!(crate::agent::hard_ask(TOOL, &json!({})).is_none());
}

#[test]
fn definitions_and_then_the_instances_of_one_are_listed_with_one_read_each_and_nothing_is_changed() {
    let (t, f) = rig();
    let defs = t.call(TOOL, &json!({})).unwrap();
    assert!(defs.contains("Dev cluster: 1 definition") && defs.contains("widgets.example.com") && defs.contains("group example.com"), "{defs}");
    let inst = t.call(TOOL, &json!({"definition": "widgets.example.com"})).unwrap();
    assert!(inst.contains("2 instances of widgets.example.com") && inst.contains("alpha in shop") && inst.contains("Ready") && inst.contains("beta in empty") && inst.contains("Not ready: Degraded"), "{inst}");
    let one = t.call(TOOL, &json!({"definition": "widgets.example.com", "namespace": "shop", "cluster": "Lab"})).unwrap();
    assert!(one.starts_with("Lab: 1 instance of widgets.example.com") && !one.contains("beta"), "{one}");
    let calls = f.kube.calls();
    assert!(calls.iter().all(|c| c.contains(" get ") || c.starts_with("get ")), "only reads: {calls:?}");
    assert!(calls.iter().all(|c| !c.contains("delete") && !c.contains("apply") && !c.contains("patch")), "{calls:?}");
    let log = f.audit.lock().unwrap().join("|");
    assert!(log.contains("k8s.custom_resources local definitions") && log.contains("k8s.custom_resources local widgets.example.com") && log.contains("k8s.custom_resources lab widgets.example.com"), "{log}");
}

#[test]
fn a_bad_name_an_unknown_cluster_an_empty_answer_and_a_cluster_error_are_said_in_words() {
    let (t, f) = rig();
    for (what, a) in [("a name with a slash", json!({"definition": "a/b"})), ("a bad namespace", json!({"definition": "widgets.example.com", "namespace": "A B"})), ("an unknown cluster", json!({"cluster": "nowhere"}))] {
        assert!(t.call(TOOL, &a).is_err(), "{what}");
    }
    assert!(t.call(TOOL, &json!({"cluster": "nowhere"})).unwrap_err().contains("Dev cluster, Lab"));
    f.kube.answer("get widgets.example.com -A -o json", &json!({"items":[]}).to_string());
    assert!(t.call(TOOL, &json!({"definition": "widgets.example.com"})).unwrap().contains("no instances of widgets.example.com"));
    f.kube.fail("get customresourcedefinitions -o json", "error: You must be logged in to the server (Unauthorized)");
    assert!(t.call(TOOL, &json!({})).is_err(), "a cluster error is an error, not an empty list");
    assert!(t.call("other", &json!({})).unwrap_err().contains("inner got other"));
    let none = K8sExpTools::new(Arc::new(Inner), Arc::new(Fake { kube: Arc::new(FakeKube::new()), names: vec![], audit: Mutex::new(vec![]) }));
    assert!(none.call(TOOL, &json!({})).unwrap_err().contains("no cluster"));
}

/// What the wrapped tools decide for themselves (a card text, a refusal, no session grant, always-ask) must still reach the chat: a wrapper that
/// forgot one of these hid the approval of the screen picture once.
#[test]
fn every_decision_of_the_tools_underneath_passes_through_unchanged() {
    struct Deciding;
    impl Tools for Deciding {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, _: &str, _: &Value) -> Result<String, String> {
            Ok("done".into())
        }
        fn describe(&self, name: &str, _: &Value) -> String {
            format!("describe {name}")
        }
        fn preview(&self, _: &str, _: &Value) -> Option<String> {
            Some("the card".into())
        }
        fn refusal(&self, _: &str, _: &Value) -> Option<String> {
            Some("no".into())
        }
        fn pre_approved(&self, _: &str) -> bool {
            true
        }
        fn approved_always(&self, _: &str) -> bool {
            true
        }
        fn grant_blocker(&self, _: &str, _: &Value) -> Option<String> {
            Some("never granted".into())
        }
        fn correct_claims(&self, _: &str) -> Option<String> {
            Some("corrected".into())
        }
        fn take_notes(&self) -> Vec<String> {
            vec!["a note".into()]
        }
        fn jobs_running(&self) -> Vec<String> {
            vec!["a job".into()]
        }
    }
    let f = Arc::new(Fake { kube: Arc::new(FakeKube::new()), names: vec![("local", "Dev")], audit: Mutex::new(vec![]) });
    let t = K8sExpTools::new(Arc::new(Deciding), f);
    let a = json!({});
    assert_eq!(t.preview("screen_capture", &a).as_deref(), Some("the card"));
    assert_eq!(t.refusal("screen_capture", &a).as_deref(), Some("no"));
    assert!(t.pre_approved("screen_capture") && t.approved_always("screen_capture"));
    assert_eq!(t.grant_blocker("screen_capture", &a).as_deref(), Some("never granted"));
    assert_eq!(t.correct_claims("x").as_deref(), Some("corrected"));
    assert_eq!(t.take_notes(), ["a note"]);
    assert_eq!(t.jobs_running(), ["a job"]);
    assert_eq!(t.describe("screen_capture", &a), "describe screen_capture");
    assert_eq!(t.call("screen_capture", &a).unwrap(), "done");
    // and this tool has no card, no refusal of its own and is not on the always-ask list
    assert!(t.preview(TOOL, &a).is_none() && t.refusal(TOOL, &a).is_none() && t.grant_blocker(TOOL, &a).is_none());
}

const MANIFEST: &str = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: app-config\n  namespace: shop\ndata:\n  a: \"1\"\n---\n# a comment only\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n  labels:\n    name: not-this\nspec:\n  replicas: 1\n---\nkind: ClusterRole\nmetadata:\n  name: reader\n";

#[test]
fn a_manifest_is_read_into_objects_and_a_bad_one_is_refused_before_anything_is_asked() {
    let o = scan_manifest(MANIFEST).unwrap();
    assert_eq!(o.iter().map(|o| (o.kind.as_str(), o.name.as_str(), o.namespace.as_str())).collect::<Vec<_>>(), vec![("ConfigMap", "app-config", "shop"), ("Deployment", "web", ""), ("ClusterRole", "reader", "")]);
    assert!(!o[0].cluster_wide() && o[2].cluster_wide());
    assert!(scan_manifest("kind: Pod\nspec: {}\n").unwrap_err().contains("needs a `kind:`"));
    assert!(scan_manifest("# nothing\n").unwrap_err().contains("no objects"));
    assert!(scan_manifest("kind: Pod\nmetadata:\n  name: Bad_Name!\n").is_err());
    assert!(scan_manifest(&"x".repeat(MAX_YAML + 1)).unwrap_err().contains("larger"));
    let many: String = (0..=MAX_OBJECTS).map(|i| format!("kind: Pod\nmetadata:\n  name: p{i}\n---\n")).collect();
    assert!(scan_manifest(&many).unwrap_err().contains("in parts"));
    assert!(scan_manifest("kind: Pod\u{0}\nmetadata:\n  name: a\n").is_err());
}

#[test]
fn applying_asks_every_time_shows_every_object_dry_runs_first_and_never_covers_a_session() {
    let (t, f) = rig();
    let d = t.defs().into_iter().find(|d| d.spec.name == APPLY).unwrap();
    assert_eq!(d.risk, Risk::Ask);
    assert!(!t.pre_approved(APPLY) && !t.approved_always(APPLY) && t.grant_blocker(APPLY, &json!({})).is_some());
    assert!(crate::agent::hard_ask(APPLY, &json!({})).is_some());
    let a = json!({"yaml": MANIFEST, "cluster": "Lab", "namespace": "shop"});
    let card = t.preview(APPLY, &a).unwrap();
    assert!(card.contains("“Lab”") && card.contains("ConfigMap app-config in shop") && card.contains("Deployment web in shop") && card.contains("ClusterRole reader  (CLUSTER-WIDE)") && card.contains("Nothing is deleted"), "{card}");
    assert!(t.refusal(APPLY, &json!({"yaml": "kind: Pod\\n"})).is_some() && t.refusal(APPLY, &json!({})).is_some());
    f.kube.answer("--dry-run=server", "configmap/app-config configured (server dry run)");
    f.kube.answer("apply -f", "configmap/app-config configured\ndeployment.apps/web created\nclusterrole.rbac.authorization.k8s.io/reader created");
    let said = t.call(APPLY, &a).unwrap();
    assert!(said.contains("Applied 3 objects to Lab") && said.contains("deployment.apps/web created"), "{said}");
    let calls = f.kube.calls();
    let first_dry = calls.iter().position(|c| c.contains("apply") && c.contains("--dry-run=server")).unwrap();
    let real = calls.iter().position(|c| c.contains("apply -n shop apply") || (c.contains(" apply -f") && !c.contains("dry-run"))).unwrap();
    assert!(first_dry < real, "{calls:?}");
    assert!(calls.iter().any(|c| c.starts_with("MANIFEST ") && c.contains("kind: Deployment")));
    assert!(calls.iter().all(|c| !c.contains("delete")));
    assert!(f.audit.lock().unwrap().iter().any(|l| l.starts_with("k8s.apply_yaml") && l.contains("3 object")));
}

#[test]
fn a_manifest_the_cluster_refuses_in_the_dry_run_changes_nothing_and_says_why() {
    let (t, f) = rig();
    f.kube.fail("--dry-run=server", "error: unknown field spec.replicaz");
    let e = t.call(APPLY, &json!({"yaml": MANIFEST})).unwrap_err();
    assert!(e.contains("nothing was changed") && e.contains("replicaz"), "{e}");
    assert!(f.kube.calls().iter().all(|c| c.contains("--dry-run=server") || c.starts_with("MANIFEST")), "no real apply ran");
    assert!(f.audit.lock().unwrap().iter().any(|l| l.contains("refused by the dry run")));
}

struct StubGate {
    answer: Mutex<Vec<String>>,
    run_it: bool,
}

impl crate::tenantconfig::clustergate::TeamGate for StubGate {
    fn through(&self, cluster_id: &str, apply: &mut dyn FnMut(Option<&str>) -> Result<String, String>) -> Option<Result<String, String>> {
        if cluster_id != "lab" {
            return None;
        }
        self.answer.lock().unwrap().push(format!("gate {cluster_id}"));
        Some(if self.run_it { apply(Some("adam")) } else { Err("waiting for approval r-1 by a tenant_admin; nothing was changed".into()) })
    }
}

#[test]
fn a_cluster_bound_to_a_team_applies_only_when_the_team_rules_say_go_and_a_free_cluster_is_not_gated() {
    let manifest = json!({"yaml": "kind: ConfigMap\nmetadata:\n  name: a\n"});
    for (run_it, cluster) in [(false, "Lab"), (true, "Lab"), (false, "Dev cluster")] {
        let (t, f) = rig();
        f.kube.answer("--dry-run=server", "ok (dry run)");
        f.kube.answer("apply -f", "configmap/a created");
        let gate = Arc::new(StubGate { answer: Mutex::new(vec![]), run_it });
        let t = t.with_gate(gate.clone());
        let a = json!({"yaml": manifest["yaml"], "cluster": cluster});
        let r = t.call(APPLY, &a);
        let real_applies = f.kube.calls().iter().filter(|c| c.contains("apply") && !c.contains("dry-run") && !c.starts_with("MANIFEST")).count();
        match (run_it, cluster) {
            (false, "Lab") => assert!(r.unwrap_err().contains("waiting for approval") && real_applies == 0),
            (true, "Lab") => assert!(r.unwrap().contains("Applied 1 object") && real_applies == 1),
            _ => assert!(r.unwrap().contains("Applied 1 object") && real_applies == 1 && gate.answer.lock().unwrap().is_empty()),
        }
    }
}
