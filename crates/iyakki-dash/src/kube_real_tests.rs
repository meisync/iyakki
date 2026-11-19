//! The `kube` data source, the default k3s dashboard and the `cluster://` lookups against the REAL kubectl and the real k3s of this
//! machine. Only read verbs are used on the cluster at large (`get`, `version`, the same fixed looks the source takes); everything
//! that is created lives in a throwaway namespace this test makes and deletes. Skipped when no cluster answers.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use super::cluster::{self, ClusterApi, KubeCluster, Kubectl};
use super::connector::*;
use super::kube::{view_names, Kube};

struct Real;
impl Real {
    fn raw(args: &[&str], stdin: Option<&str>) -> Result<String, String> {
        let mut c = Command::new("kubectl");
        c.arg("--request-timeout=30s").args(args).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut ch = c.spawn().map_err(|e| e.to_string())?;
        if let Some(s) = stdin {
            ch.stdin.take().ok_or("no stdin")?.write_all(s.as_bytes()).map_err(|e| e.to_string())?;
        }
        let o = ch.wait_with_output().map_err(|e| e.to_string())?;
        if o.status.success() {
            Ok(String::from_utf8_lossy(&o.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&o.stderr).into_owned())
        }
    }
}
impl Kubectl for Real {
    fn kubectl(&self, args: &[&str]) -> Result<String, String> {
        Real::raw(args, None)
    }
}

struct Ns(String);
impl Drop for Ns {
    fn drop(&mut self) {
        let _ = Real::raw(&["delete", "namespace", &self.0, "--wait=false"], None);
    }
}

const OBJECTS: &str = r#"
apiVersion: apps/v1
kind: Deployment
metadata: {name: dep-bad}
spec:
  replicas: 1
  selector: {matchLabels: {app: bad}}
  template:
    metadata: {labels: {app: bad}}
    spec:
      containers: [{name: c, image: "iyakki-test.invalid/none:1"}]
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata: {name: claim-wait}
spec:
  accessModes: [ReadWriteOnce]
  resources: {requests: {storage: 1Mi}}
---
apiVersion: v1
kind: Service
metadata: {name: prometheus-noready, labels: {app.kubernetes.io/name: prometheus}}
spec:
  selector: {app: nothing-matches}
  ports: [{port: 9090}]
---
apiVersion: v1
kind: Service
metadata: {name: prometheus-ready}
spec:
  ports: [{port: 9090}]
---
apiVersion: v1
kind: Endpoints
metadata: {name: prometheus-ready}
subsets: [{addresses: [{ip: 10.255.255.1}], ports: [{port: 9090}]}]
"#;

#[test]
fn the_kube_source_the_default_dashboard_and_cluster_addresses_work_against_the_real_k3s() {
    if Real::raw(&["version", "--request-timeout=5s"], None).is_err() {
        eprintln!("skipped: no cluster answers on this machine");
        return;
    }
    let mut r = [0u8; 4];
    let _ = iyakki_sec::random_fill(&mut r);
    let ns = format!("iyakki-test-{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>());
    Real::raw(&["create", "namespace", &ns], None).unwrap();
    let _guard = Ns(ns.clone());
    Real::raw(&["-n", &ns, "apply", "-f", "-"], Some(OBJECTS)).unwrap();

    let api: Arc<dyn ClusterApi> = Arc::new(KubeCluster::new(Arc::new(Real), Duration::from_secs(1)));
    let src = Kube::with_api("kube-local", api.clone());
    let range = TimeRange::last(3_600_000, iyakki_clock::now_unix() as i64 * 1000);
    let ask = |text: &str| src.query(&QueryRequest::new(text, range.clone()), &Cancel::new());

    // every view answers from the real cluster (never an error, also where metrics-server is missing)
    for v in view_names() {
        let f = ask(v).unwrap_or_else(|e| panic!("view {v}: {} {}", e.message, e.fix.clone().unwrap_or_default()));
        assert!(!f.columns.is_empty(), "view {v} has columns");
    }
    let cluster_view = ask("cluster").unwrap();
    assert!(cluster_view.cell(0, 0).to_ascii_lowercase().contains("run") || cluster_view.cell(0, 0).to_ascii_lowercase().contains("ok") || !cluster_view.cell(0, 0).is_empty(), "{:?}", cluster_view.rows);
    assert!(ask("nodes").unwrap().rows.len() >= 1);
    assert!(ask("pods_by_namespace").unwrap().rows.iter().any(|r| r.iter().any(|c| c.as_str() == Some("kube-system"))), "kube-system has pods");
    // things this test made show up in the right views (pod problems take a moment: the image cannot be pulled)
    let mut seen = false;
    for _ in 0..40 {
        let p = ask("pod_problems").unwrap();
        if p.rows.iter().any(|r| r.iter().any(|c| c.as_str() == Some(ns.as_str()))) {
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    assert!(seen, "a pod that cannot pull its image is listed in pod_problems");
    assert!(ask("deployments_unavailable").unwrap().rows.iter().any(|r| r.iter().any(|c| c.as_str() == Some("dep-bad"))));
    assert!(ask("pvcs").unwrap().rows.iter().any(|r| r.iter().any(|c| c.as_str() == Some("claim-wait"))));
    // a write is refused before kubectl is reached
    assert_eq!(ask("delete pods --all").unwrap_err().kind, ErrKind::ReadOnly);
    // the default dashboard: every panel's query runs against the real cluster
    let d = super::templates::find("k3s-local").unwrap().instantiate("kube-local", "k3s-local");
    assert!(d.panels.len() >= 20);
    for p in &d.panels {
        let f = ask(&p.query.text).unwrap_or_else(|e| panic!("panel {} ({}): {}", p.id, p.query.text, e.message));
        assert!(!f.columns.is_empty());
    }

    // cluster:// addresses through the real Service and Endpoints objects
    let ok = cluster::parse(&format!("cluster://{ns}/prometheus-ready:9090")).unwrap().unwrap();
    let url = cluster::resolve(api.as_ref(), &ok).unwrap();
    assert!(url.starts_with("http://10.") && url.ends_with(":9090"), "{url}");
    let notready = cluster::parse(&format!("cluster://{ns}/prometheus-noready:9090")).unwrap().unwrap();
    assert!(cluster::resolve(api.as_ref(), &notready).unwrap_err().message.contains("no ready pod"));
    let missing = cluster::parse(&format!("cluster://{ns}/nothing:9090")).unwrap().unwrap();
    assert!(cluster::resolve(api.as_ref(), &missing).unwrap_err().message.contains("no Service"));
    let wrong_port = cluster::parse(&format!("cluster://{ns}/prometheus-ready:1234")).unwrap().unwrap();
    assert!(cluster::resolve(api.as_ref(), &wrong_port).unwrap_err().message.contains("no port 1234"));
    // the Add source form's lookup finds a Prometheus-looking Service by name, label and port in the real cluster
    let list = api.services().unwrap();
    let found: Vec<_> = cluster::candidates(&list).into_iter().filter(|c| c.ns == ns).collect();
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.iter().any(|c| c.name == "prometheus-ready" && c.ready) && found.iter().any(|c| c.name == "prometheus-noready" && !c.ready));
}
