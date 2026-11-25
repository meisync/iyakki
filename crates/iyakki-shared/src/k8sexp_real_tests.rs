//! The K8S tile's explorer code against the REAL kubectl and the real cluster of this machine, inside a throwaway namespace this
//! test creates and deletes itself. Only that namespace is read or written. Skipped (passes with a note) when no cluster answers.

use crate::k8s::*;
use crate::k8sexp::*;
use std::io::Write;
use std::process::{Command, Stdio};

fn kc(args: &[&str], stdin: Option<&str>) -> Option<(bool, String)> {
    let mut c = Command::new("kubectl");
    c.args(["--request-timeout=20s"]).args(args).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut ch = c.spawn().ok()?;
    if let Some(s) = stdin {
        ch.stdin.take()?.write_all(s.as_bytes()).ok()?;
    }
    let o = ch.wait_with_output().ok()?;
    Some((o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))))
}

struct Ns(String);
impl Drop for Ns {
    fn drop(&mut self) {
        let _ = kc(&["delete", "namespace", &self.0, "--wait=false"], None);
    }
}

const OBJECTS: &str = r#"
apiVersion: v1
kind: ConfigMap
metadata: {name: cm1}
data: {hello: world}
---
apiVersion: v1
kind: Secret
metadata: {name: sec1}
type: Opaque
stringData: {password: hunter2-very-secret}
---
apiVersion: v1
kind: ServiceAccount
metadata: {name: sa1}
---
apiVersion: v1
kind: Service
metadata: {name: svc1}
spec:
  selector: {app: demo}
  ports: [{port: 80, targetPort: 8080}]
---
apiVersion: apps/v1
kind: Deployment
metadata: {name: dep1}
spec:
  replicas: 1
  selector: {matchLabels: {app: demo}}
  template:
    metadata: {labels: {app: demo}}
    spec:
      containers: [{name: c, image: "iyakki-test.invalid/none:1"}]
"#;

#[test]
fn the_explorer_lists_details_masks_secrets_and_stays_inside_one_real_namespace() {
    if !kc(&["version", "--request-timeout=5s"], None).is_some_and(|(ok, _)| ok) {
        eprintln!("skipped: no cluster answers on this machine");
        return;
    }
    let mut r = [0u8; 4];
    let _ = iyakki_sec::random_fill(&mut r);
    let name = format!("iyakki-test-{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>());
    let (ok, out) = kc(&["create", "namespace", &name], None).unwrap();
    assert!(ok, "{out}");
    let _guard = Ns(name.clone());
    let (ok, out) = kc(&["-n", &name, "apply", "-f", "-"], Some(OBJECTS)).unwrap();
    assert!(ok, "{out}");

    // the runner starts kubectl with a cut-down environment, so the kubeconfig the test itself used is named in the target
    let tg = Target { kubeconfig: std::env::var("KUBECONFIG").unwrap_or_default(), ..Target::default() };
    let scope = Scope::Ns(name.clone());
    let now = iyakki_clock::now_unix() as u64;
    let names = |k: Kind| -> Vec<String> { list(&crate::k8s::Kubectl, &tg, &scope, k, now).rows.iter().map(|r| r.name.clone()).collect() };
    // every namespaced kind lists without an error from the real server
    for k in Kind::ALL.iter().copied().filter(|k| k.namespaced()) {
        let l = list(&crate::k8s::Kubectl, &tg, &scope, k, now);
        assert_eq!(l.cluster, Cluster::Ok, "{k:?}: {:?}", l.cluster);
        assert!(l.rows.iter().all(|r| r.namespace == name || r.namespace.is_empty()), "{k:?} stays in the namespace");
    }
    assert_eq!(names(Kind::ConfigMaps).iter().filter(|n| *n == "cm1").count(), 1, "{:?}", list(&crate::k8s::Kubectl, &tg, &scope, Kind::ConfigMaps, now));
    assert_eq!(names(Kind::Secrets), vec!["sec1"]);
    assert_eq!(names(Kind::Services), vec!["svc1"]);
    assert!(names(Kind::ServiceAccounts).contains(&"sa1".to_string()));
    assert_eq!(names(Kind::Deployments), vec!["dep1"]);
    let deps = list(&crate::k8s::Kubectl, &tg, &scope, Kind::Deployments, now);
    assert_eq!(deps.total, 1);
    assert!(count_line(Kind::Deployments, &deps.rows, deps.total).contains('1'));
    // a Secret: the listing never carries the value, the details show dots, only a reveal shows the text
    let secrets = list(&crate::k8s::Kubectl, &tg, &scope, Kind::Secrets, now);
    assert!(!format!("{:?}", secrets.rows).contains("hunter2"), "a list never holds secret values");
    let mut obj = get_object(&crate::k8s::Kubectl, &tg, Kind::Secrets, &name, "sec1").unwrap();
    let masked = object_yaml(Kind::Secrets, &obj, false);
    assert!(masked.contains(MASK) && !masked.contains("hunter2") && !masked.contains("aHVudGVy"), "{masked}");
    assert!(!masked.contains("last-applied"), "the annotation that copies the values is removed");
    assert!(object_yaml(Kind::Secrets, &obj, true).contains("hunter2-very-secret"), "a reveal shows the text");
    scrub_secret(&mut obj);
    assert!(!obj.to_string().contains("hunter2") && !obj.to_string().contains("aHVudGVy"));
    // details of a ConfigMap: summary and YAML from the real answer
    let cm = get_object(&crate::k8s::Kubectl, &tg, Kind::ConfigMaps, &name, "cm1").unwrap();
    let y = object_yaml(Kind::ConfigMaps, &cm, false);
    assert!(y.contains("hello: world") && y.contains("name: cm1") && !y.contains("managedFields"), "{y}");
    assert!(!summary(Kind::ConfigMaps, &cm, now).is_empty());
    // the pod the Deployment makes cannot pull its image: a real failing pod shows up as a problem row
    let mut bad = false;
    // up to 4 minutes: the first pull attempt and its failure take a while on a cluster that other builds are using
    for _ in 0..120 {
        let l = list(&crate::k8s::Kubectl, &tg, &scope, Kind::Pods, now);
        if l.rows.iter().any(|r| r.health == Some(Health::Bad) || r.cells.iter().any(|c| c.contains("ImagePull") || c.contains("ErrImage"))) {
            bad = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    assert!(bad, "a pod that cannot pull its image is shown as a problem");
    // an object that is not there, and a name that could become an option, are plain errors
    assert!(get_object(&crate::k8s::Kubectl, &tg, Kind::ConfigMaps, &name, "nothing-here").is_err());
    assert!(get_object(&crate::k8s::Kubectl, &tg, Kind::ConfigMaps, &name, "--all").is_err());
}
