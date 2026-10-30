use super::*;
use std::cell::RefCell;

/// A fake cluster: the claims that exist (name -> (size, class)), the classes (name, default, expandable), every call.
struct Cluster {
    pvcs: RefCell<Vec<(String, String, String)>>,
    classes: Vec<(&'static str, bool, bool)>,
    calls: RefCell<Vec<String>>,
}

impl Cluster {
    fn new(pvcs: &[(&str, &str, &str)], classes: &[(&'static str, bool, bool)]) -> Cluster {
        Cluster { pvcs: RefCell::new(pvcs.iter().map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string())).collect()), classes: classes.to_vec(), calls: RefCell::new(vec![]) }
    }
    fn run(&self, a: &[&str]) -> KOut {
        self.calls.borrow_mut().push(a.join(" "));
        let ok = |s: String| KOut { ok: true, stdout: s.clone(), text: s };
        match a {
            ["get", "pvc", name, ..] => match self.pvcs.borrow().iter().find(|p| p.0 == *name) {
                Some(p) => ok(format!("{{\"spec\":{{\"storageClassName\":\"{}\",\"resources\":{{\"requests\":{{\"storage\":\"{}\"}}}}}}}}", p.2, p.1)),
                None => KOut { ok: false, stdout: String::new(), text: format!("Error from server (NotFound): persistentvolumeclaims \"{name}\" not found") },
            },
            ["get", "storageclass", "-o", "json"] => ok(format!("{{\"items\":[{}]}}", self.classes.iter().map(|c| format!("{{\"metadata\":{{\"name\":\"{}\",\"annotations\":{{\"storageclass.kubernetes.io/is-default-class\":\"{}\"}}}}}}", c.0, c.1)).collect::<Vec<_>>().join(","))),
            ["get", "storageclass", name, "-o", "json"] => ok(format!("{{\"allowVolumeExpansion\":{}}}", self.classes.iter().find(|c| c.0 == *name).is_some_and(|c| c.2))),
            ["patch", "pvc", name, ..] => {
                let mut p = self.pvcs.borrow_mut();
                if let Some(x) = p.iter_mut().find(|x| x.0 == *name) {
                    x.1 = a.last().unwrap_or(&"").split('"').nth(7).unwrap_or("").to_string();
                }
                ok("patched".into())
            }
            _ => KOut { ok: false, stdout: String::new(), text: "unexpected".into() },
        }
    }
}

const PVC: &str = "apiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: gitea-data\n  namespace: iyakki-gitea\nspec:\n  accessModes:\n  - ReadWriteOnce\n  resources:\n    requests:\n      storage: 10Gi\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: gitea\n  namespace: iyakki-gitea\nspec:\n  replicas: 1\n";

#[test]
fn an_existing_bound_claim_is_left_out_of_the_apply() {
    let c = Cluster::new(&[("gitea-data", "10Gi", "local-path")], &[("local-path", true, false)]);
    let g = guard(PVC, None, &|a| c.run(a)).unwrap();
    assert!(!g.manifest.contains("PersistentVolumeClaim") && g.manifest.contains("kind: Deployment"), "{}", g.manifest);
    assert!(g.notes.iter().any(|n| n.contains("gitea-data exists already") && n.contains("local-path") && n.contains("data stays")), "{:?}", g.notes);
    assert!(c.calls.borrow().iter().all(|x| !x.contains("patch")));
}

#[test]
fn a_new_claim_is_created_with_the_default_class_written_out() {
    let c = Cluster::new(&[], &[("other", false, false), ("fast", true, false)]);
    let g = guard(PVC, None, &|a| c.run(a)).unwrap();
    assert!(g.manifest.contains("spec:\n  storageClassName: fast\n  accessModes"), "{}", g.manifest);
    // without any default, local-path
    let c = Cluster::new(&[], &[("local-path", false, false)]);
    assert!(guard(PVC, None, &|a| c.run(a)).unwrap().manifest.contains("storageClassName: local-path"));
    // a class the manifest names is kept
    let named = PVC.replace("spec:\n  accessModes", "spec:\n  storageClassName: mine\n  accessModes");
    assert_eq!(guard(&named, None, &|a| c.run(a)).unwrap().manifest.matches("storageClassName").count(), 1);
}

#[test]
fn a_bigger_size_is_resized_only_when_the_class_allows_it() {
    let big = PVC.replace("10Gi", "20Gi");
    let c = Cluster::new(&[("gitea-data", "10Gi", "local-path")], &[("local-path", true, true)]);
    let g = guard(&big, None, &|a| c.run(a)).unwrap();
    assert!(g.notes.iter().any(|n| n.contains("resized")), "{:?}", g.notes);
    assert!(c.calls.borrow().iter().any(|x| x.starts_with("patch pvc gitea-data")));
    assert!(!g.manifest.contains("PersistentVolumeClaim"));
    let c = Cluster::new(&[("gitea-data", "10Gi", "local-path")], &[("local-path", true, false)]);
    let g = guard(&big, None, &|a| c.run(a)).unwrap();
    assert!(g.notes.iter().any(|n| n.contains("does not allow growing") && n.contains("stays 10Gi")), "{:?}", g.notes);
    assert!(c.calls.borrow().iter().all(|x| !x.starts_with("patch")));
    // smaller
    let small = PVC.replace("10Gi", "5Gi");
    let g = guard(&small, None, &|a| c.run(a)).unwrap();
    assert!(g.notes.iter().any(|n| n.contains("cannot shrink")), "{:?}", g.notes);
}

#[test]
fn a_manifest_of_only_claims_applies_nothing_and_an_unreachable_cluster_is_an_error() {
    let c = Cluster::new(&[("gitea-data", "10Gi", "local-path")], &[]);
    let only = PVC.split("---\n").next().unwrap();
    let ran = RefCell::new(false);
    let out = guarded_apply(only, None, &|a| c.run(a), &|_| {
        *ran.borrow_mut() = true;
        Ok(String::new())
    })
    .unwrap();
    assert!(!*ran.borrow() && out.contains("exists already"));
    let down = |_: &[&str]| KOut { ok: false, stdout: String::new(), text: "The connection to the server was refused".into() };
    assert!(guard(PVC, None, &down).unwrap_err().contains("could not check the volume gitea-data"));
}

#[test]
fn the_immutable_refusal_is_explained() {
    let t = "kubectl apply failed: The PersistentVolumeClaim \"gitea-data\" is invalid: spec: Forbidden: spec is immutable after creation";
    let e = explain_apply_error(t);
    assert!(e.contains("gitea-data") && e.contains("cannot be changed after it is created") && e.contains("your data was not touched") && e.contains("Original message"), "{e}");
    assert_eq!(explain_apply_error("boom"), "boom");
    assert_eq!(quantity("5Gi"), Some(5.0 * 1024f64.powi(3)));
}

/// Every manifest of the crate that holds a claim: a re-run over an existing claim never applies it, a first run writes the class.
#[test]
fn every_installer_manifest_is_idempotent_over_existing_claims() {
    use crate::quickinstall as q;
    let gitea = q::gitea::with_namespace(&q::gitea::build(3300, "gitea-test").unwrap().0.manifest, q::gitea::NAMESPACE);
    let manifests: Vec<(&str, String)> = vec![
        ("gitea", gitea),
        ("clickhouse", q::clickhouse::manifest()),
        ("opensearch", q::opensearch::manifest()),
        ("matrix", q::matrix::manifest()),
        ("elasticsearch", q::elasticsearch_manifest()),
        ("graphdb", q::graphdb_manifest()),
        ("prometheus", q::prometheus_manifest()),
        ("lemonade", q::lemonade_manifest()),
        ("ollama-gpu", crate::gpu_workloads::ollama_gpu_manifest(crate::gpu_workloads::Gpu::Vulkan)),
    ];
    for (name, m) in manifests {
        let claims: Vec<String> = split_docs(&m).iter().filter(|d| kind_of(d) == "PersistentVolumeClaim").filter_map(|d| meta(d, "name")).collect();
        assert!(!claims.is_empty(), "{name} has a claim");
        // first run: nothing exists, every claim carries a class
        let fresh = Cluster::new(&[], &[("local-path", true, false)]);
        let first = guard(&m, None, &|a| fresh.run(a)).unwrap();
        let docs = split_docs(&first.manifest);
        assert_eq!(docs.iter().filter(|d| kind_of(d) == "PersistentVolumeClaim" && d.contains("storageClassName: local-path")).count(), claims.len(), "{name}:\n{}", first.manifest);
        // second run: every claim exists bound on local-path; none is applied, the rest still is
        let there: Vec<(&str, &str, &str)> = claims.iter().map(|c| (c.as_str(), "1Ti", "local-path")).collect();
        let c = Cluster::new(&there, &[("local-path", true, false)]);
        let again = guard(&m, None, &|a| c.run(a)).unwrap();
        assert!(!again.manifest.contains("PersistentVolumeClaim"), "{name}");
        assert!(again.manifest.contains("kind: Deployment"), "{name}");
        assert_eq!(again.notes.len(), claims.len(), "{name}: {:?}", again.notes);
        assert!(c.calls.borrow().iter().all(|x| !x.starts_with("patch")), "{name}");
    }
}

#[test]
fn cluster_items_are_healthy_from_the_deployments_the_cluster_lists() {
    use crate::quickinstall::{healthy_from, Item};
    let j = r#"{"items":[{"metadata":{"namespace":"iyakki-gitea","name":"gitea"},"spec":{"replicas":1},"status":{"readyReplicas":1}},{"metadata":{"namespace":"iyakki-matrix","name":"matrix"},"spec":{"replicas":1},"status":{}},{"metadata":{"namespace":"iyakki-data","name":"clickhouse"},"spec":{"replicas":0},"status":{}}]}"#;
    assert_eq!(healthy_from(j), vec![Item::Gitea]);
    assert!(healthy_from("not json").is_empty());
}
