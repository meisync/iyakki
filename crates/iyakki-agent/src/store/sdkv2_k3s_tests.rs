use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct Fake {
    up: bool,
    namespaces: Mutex<Vec<String>>,
    calls: Mutex<Vec<String>>,
    files: Mutex<Vec<(String, String)>>,
    answers: Mutex<Vec<(String, Result<String, String>)>>,
}

impl Fake {
    fn running() -> Arc<Fake> {
        Arc::new(Fake { up: true, ..Default::default() })
    }
    fn answer(&self, contains: &str, r: Result<&str, &str>) {
        self.answers.lock().unwrap().push((contains.into(), r.map(str::to_string).map_err(str::to_string)));
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl KubePort for Fake {
    fn running(&self) -> bool {
        self.up
    }
    fn kubectl(&self, args: &[String]) -> Result<String, String> {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        if let Some((_, r)) = self.answers.lock().unwrap().iter().find(|(c, _)| line.contains(c)) {
            return r.clone();
        }
        if line.starts_with("get namespace ") {
            let ns = args[2].clone();
            return if self.namespaces.lock().unwrap().contains(&ns) { Ok(format!("namespace/{ns}")) } else { Err("Error from server (NotFound): namespaces not found".into()) };
        }
        if line.starts_with("apply -f") && self.files.lock().unwrap().last().is_some_and(|f| f.1.contains("kind: Namespace")) {
            let ns = self.files.lock().unwrap().last().unwrap().0.trim_end_matches("-namespace").to_string();
            self.namespaces.lock().unwrap().push(namespace_of(&ns));
        }
        Ok(String::new())
    }
    fn write_manifest(&self, app: &str, text: &str) -> Result<String, String> {
        self.files.lock().unwrap().push((app.to_string(), text.to_string()));
        Ok(format!("/m/{app}.yaml"))
    }
    fn is_local_image(&self, image: &str) -> bool {
        image.starts_with("localhost/")
    }
}

fn who(app: &str) -> Who {
    Who { app: app.into(), user: "pat".into() }
}

const OK_YAML: &str = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\nspec:\n  replicas: 1\n  selector:\n    matchLabels:\n      app: web\n  template:\n    metadata:\n      labels:\n        app: web\n    spec:\n      containers:\n        - name: web\n          image: docker.io/library/nginx:1.27\n          resources:\n            limits:\n              cpu: 100m\n              memory: 64Mi\n          securityContext:\n            runAsNonRoot: true\n            runAsUser: 1000\n";

#[test]
fn every_app_has_its_own_dns_safe_namespace() {
    assert_eq!(namespace_of("notes"), "app-notes");
    assert_eq!(namespace_of("My_App.v2"), "app-my-app-v2");
    assert_eq!(namespace_of("--x--"), "app-x");
    assert!(namespace_of(&"a".repeat(200)).len() <= 63);
    assert_ne!(namespace_of("a"), namespace_of("b"));
}

#[test]
fn a_cluster_that_is_not_running_is_unavailable_with_the_fix_and_a_plan_needs_no_cluster() {
    let e = K3sEngine { kube: Arc::new(Fake::default()) };
    for op in ["k3s.get", "k3s.logs", "k3s.apply", "k3s.restart"] {
        let err = e.call(&who("notes"), op, &json!({"kind": "pods", "pod": "p", "yaml": OK_YAML, "name": "web"})).unwrap_err();
        assert_eq!(err.code, code::UNAVAILABLE, "{op}");
        assert!(err.message.contains("not running"), "{}", err.message);
    }
    let st = e.call(&who("notes"), "k3s.status", &json!({})).unwrap();
    assert_eq!((st["ready"].clone(), st["namespace"].clone()), (json!(false), json!("app-notes")));
    let p = e.call(&who("notes"), "k3s.plan", &json!({"yaml": OK_YAML})).unwrap();
    assert!(p["plan"].as_str().unwrap().contains("Deployment web") && p["refused"].as_array().unwrap().is_empty(), "{p}");
}

#[test]
fn reads_are_in_the_apps_own_namespace_only_and_a_namespace_that_is_not_there_is_an_empty_list() {
    let k = Fake::running();
    let e = K3sEngine { kube: k.clone() };
    assert_eq!(e.call(&who("notes"), "k3s.get", &json!({"kind": "pods"})).unwrap(), json!({"items": []}), "no namespace yet: nothing to list, and nothing made by a read");
    assert!(k.namespaces.lock().unwrap().is_empty());
    k.namespaces.lock().unwrap().push("app-notes".into());
    k.answer("get pods", Ok(r#"{"items":[{"metadata":{"name":"web-1"}}]}"#));
    let v = e.call(&who("notes"), "k3s.get", &json!({"kind": "pods", "selector": "app=web"})).unwrap();
    assert_eq!(v["items"][0]["metadata"]["name"], "web-1");
    assert!(k.calls().last().unwrap().contains("-l app=web -n app-notes -o json"), "{:?}", k.calls());
    // one named object is a list of one
    k.answer("get deployments web", Ok(r#"{"metadata":{"name":"web"}}"#));
    assert_eq!(e.call(&who("notes"), "k3s.get", &json!({"kind": "deployments", "name": "web"})).unwrap()["items"][0]["metadata"]["name"], "web");
    k.answer("logs web-1", Ok("one\ntwo\nthree\n"));
    assert_eq!(e.call(&who("notes"), "k3s.logs", &json!({"pod": "web-1", "tail": 2})).unwrap()["lines"], json!(["one", "two", "three"]));
    assert!(k.calls().iter().any(|c| c == "logs web-1 -n app-notes --tail=2"), "{:?}", k.calls());
    // another app's name never reaches the kubectl line
    for c in k.calls() {
        assert!(!c.contains("app-other"), "{c}");
    }
}

#[test]
fn the_plan_is_the_deploy_lint_and_refuses_what_the_cluster_rules_refuse() {
    let e = K3sEngine { kube: Fake::running() };
    let bad = "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: other\n---\napiVersion: v1\nkind: Pod\nmetadata:\n  name: p\n  namespace: app-someone-else\nspec:\n  hostNetwork: true\n  containers:\n    - name: c\n      image: busybox\n      securityContext:\n        privileged: true\n";
    let p = e.call(&who("notes"), "k3s.plan", &json!({"yaml": bad})).unwrap();
    let refused: Vec<String> = p["refused"].as_array().unwrap().iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
    assert!(refused.len() >= 3, "{refused:?}");
    assert!(refused.iter().any(|r| r.contains("Namespace")) && refused.iter().any(|r| r.contains("privileged")) && refused.iter().any(|r| r.to_lowercase().contains("namespace")), "{refused:?}");
    assert_eq!(p["plan"], "nothing would be applied");
}

#[test]
fn the_first_change_makes_the_namespace_with_its_quota_and_default_deny_and_applies_the_lints_own_text() {
    let k = Fake::running();
    let e = K3sEngine { kube: k.clone() };
    let r = e.call(&who("notes"), "k3s.apply", &json!({"yaml": OK_YAML})).unwrap();
    assert_eq!(r["applied"], 1);
    let files = k.files.lock().unwrap().clone();
    assert_eq!(files.len(), 2, "the namespace file, then the app's own");
    assert!(files[0].1.contains("kind: Namespace") && files[0].1.contains("iyakki.app: \"notes\"") && files[0].1.contains("kind: ResourceQuota") && files[0].1.contains("pods: \"10\"") && files[0].1.contains("default-deny-ingress"));
    assert!(files[1].1.contains("kind: Deployment") && !files[1].1.contains("kind: Namespace"));
    assert_eq!(k.calls().last().unwrap(), "apply -f /m/notes.yaml -n app-notes");
    // the second change finds the namespace and does not make it again
    e.call(&who("notes"), "k3s.apply", &json!({"yaml": OK_YAML})).unwrap();
    assert_eq!(k.files.lock().unwrap().len(), 3);
    // a refused plan applies nothing
    let before = k.calls().len();
    let err = e.call(&who("notes"), "k3s.apply", &json!({"yaml": "apiVersion: v1\nkind: ClusterRole\nmetadata:\n  name: x\n"})).unwrap_err();
    assert_eq!(err.code, code::BLOCKED);
    assert_eq!(k.calls().len(), before, "kubectl was not called");
}

#[test]
fn a_restart_finds_the_workload_kind_and_says_plainly_when_there_is_none() {
    let k = Fake::running();
    let e = K3sEngine { kube: k.clone() };
    k.answer("deployment/web", Err("Error from server (NotFound): deployments.apps \"web\" not found"));
    k.answer("statefulset/web", Ok("restarted"));
    assert_eq!(e.call(&who("notes"), "k3s.restart", &json!({"name": "web"})).unwrap()["restarted"], "web");
    assert_eq!(k.calls().iter().filter(|c| c.starts_with("rollout restart")).count(), 2);
    let k2 = Fake::running();
    for kind in ["deployment", "statefulset", "daemonset"] {
        k2.answer(&format!("{kind}/gone"), Err("NotFound"));
    }
    let err = K3sEngine { kube: k2 }.call(&who("notes"), "k3s.restart", &json!({"name": "gone"})).unwrap_err();
    assert!(err.message.contains("no workload called gone"), "{}", err.message);
}

#[test]
fn the_real_port_writes_the_manifest_for_the_person_only() {
    use std::os::unix::fs::PermissionsExt;
    let d = std::env::temp_dir().join(format!("iyakki-k3s-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let svc = crate::services::Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let r = RealKube { dev: svc.k3sdev.clone(), dir: d.clone() };
    assert!(!r.running(), "no cluster is installed in a fresh data folder");
    let f = r.write_manifest("not/es", "kind: X\n").unwrap();
    assert!(f.ends_with("appk3s/notes.yaml"), "{f}");
    assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "kind: X\n");
    let _ = std::fs::remove_dir_all(&d);
}

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}
