//! The Prometheus item with fakes only: no cluster, no network. Manifest text, dependency on k3s, the DASH source, failure text.

use super::*;

#[derive(Default)]
struct Fake {
    lines: Vec<String>,
    calls: Vec<String>,
    k3s: bool,
    apply_fails: bool,
    rollout_fails: bool,
    register_fails: bool,
    registered: Vec<String>,
    marks: Vec<(Item, String)>,
    deps: Vec<(Item, DepState)>,
}

impl StepEnv for Fake {
    fn say(&mut self, line: &str) {
        self.lines.push(line.into());
    }
    fn dep_state(&mut self, dep: Item) -> DepState {
        self.deps.iter().find(|d| d.0 == dep).map_or(DepState::Ready, |d| d.1.clone())
    }
    fn k3s_present(&mut self) -> bool {
        self.k3s
    }
    fn k3s_elsewhere(&mut self) -> Option<String> {
        None
    }
    fn install_k3s(&mut self) -> Result<String, String> {
        Err("not used".into())
    }
    fn request_k3s_start(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn cluster_ready(&mut self) -> bool {
        self.k3s
    }
    fn install_cli(&mut self, _id: &str) -> Result<String, String> {
        Err("not used".into())
    }
    fn kubectl_apply(&mut self, name: &str, manifest: &str) -> Result<String, String> {
        self.calls.push(format!("apply {name}"));
        assert_eq!(manifest, prometheus_manifest());
        if self.apply_fails { Err("kubectl apply failed: forbidden".into()) } else { Ok("namespace/iyakki-monitoring created\ndeployment.apps/prometheus created".into()) }
    }
    fn rollout(&mut self, ns: &str, name: &str, _t: Duration) -> Result<(), String> {
        self.calls.push(format!("rollout {ns}/{name}"));
        if self.rollout_fails { Err("prometheus did not become ready in 10 minutes: ImagePullBackOff".into()) } else { Ok(()) }
    }
    fn sleep(&mut self, _d: Duration) {}
    fn mark(&mut self, item: Item, detail: &str) -> Result<(), String> {
        self.marks.push((item, detail.into()));
        Ok(())
    }
    fn request_after(&mut self, _what: &str) -> Result<(), String> {
        Ok(())
    }
    fn lemonade_ready(&mut self) -> bool {
        false
    }
    fn lemonade_pull(&mut self, _model: &str) -> Result<(), String> {
        Err("not used".into())
    }
    fn register_dash_source(&mut self, cfg: crate::dash::connector::SourceConfig) -> Result<(), String> {
        if self.register_fails {
            return Err("at most 50 sources".into());
        }
        self.registered.push(format!("{}={}", cfg.id, cfg.value("url")));
        Ok(())
    }
}

fn run(f: &mut Fake) -> Result<(), String> {
    run_step(Item::Prometheus, f, Duration::from_secs(60), Duration::from_secs(5))
}

#[test]
fn the_item_is_a_cluster_item_that_needs_k3s_and_orders_after_it() {
    assert_eq!(Item::parse("prometheus"), Some(Item::Prometheus));
    assert_eq!(Item::Prometheus.group(), Group::Cluster);
    assert_eq!(Item::Prometheus.needs(), &[Item::K3s]);
    assert!(Item::Prometheus.is_service());
    assert!(!Item::Prometheus.is_hardware());
    assert!(Item::Prometheus.licence().contains("Apache-2.0") && Item::Prometheus.licence().contains("does not ship"));
    assert_eq!(order(Item::Prometheus, &|_| false), vec![Item::K3s, Item::Prometheus]);
    assert_eq!(order(Item::Prometheus, &|i| i == Item::K3s), vec![Item::Prometheus]);
    assert_eq!(label(Item::Prometheus, 2, 2), "Install Prometheus (metrics) (2/2, after k3s)");
}

#[test]
fn the_image_comes_from_the_vendor_registry_only_and_is_pinned_in_one_place() {
    assert!(PROM_IMAGE.starts_with("quay.io/prometheus/prometheus:v"), "{PROM_IMAGE}");
    assert!(!PROM_IMAGE.ends_with(":latest"));
    let m = prometheus_manifest();
    let images: Vec<&str> = m.lines().filter_map(|l| l.trim().strip_prefix("image: ")).collect();
    assert_eq!(images, [PROM_IMAGE], "one image, nothing else is pulled");
}

#[test]
fn the_manifest_is_unprivileged_read_only_in_the_cluster_and_not_exposed() {
    let m = prometheus_manifest();
    assert!(m.contains("allowPrivilegeEscalation: false") && m.contains("readOnlyRootFilesystem: true") && m.contains("drop: [ALL]") && m.contains("runAsNonRoot: true"));
    assert!(!m.contains("privileged") && !m.contains("hostPath") && !m.contains("hostNetwork") && !m.contains("hostPID"), "no privileged pod, no host access");
    assert!(m.contains("type: ClusterIP") && !m.contains("NodePort") && !m.contains("LoadBalancer") && !m.contains("hostPort"));
    assert!(m.contains("limits:") && m.contains("memory: 768Mi") && m.contains("cpu: 500m") && m.contains("requests:"));
    assert!(m.contains("claimName: prometheus-data") && m.contains("storage: 5Gi") && m.contains("retention.size=4GB"));
    // RBAC: only get, list and watch; no wildcard, no proxy, no secrets
    let rules = m.split("kind: ClusterRole\n").nth(1).unwrap().split("---").next().unwrap();
    for bad in ["create", "update", "patch", "delete", "deletecollection", "\"*\"", "'*'", "[*]", "nodes/proxy", "secrets"] {
        assert!(!rules.contains(bad), "ClusterRole has {bad}: {rules}");
    }
    assert_eq!(rules.matches("verbs: [get, list, watch]").count(), 1);
    assert!(rules.contains("verbs: [get]"));
    assert!(m.contains("kind: ClusterRoleBinding") && m.contains("kind: ServiceAccount") && m.contains("serviceAccountName: prometheus"));
    assert!(!m.contains("cluster-admin"));
    // the scrape config covers the nodes and the pods that ask for it
    assert!(m.contains("role: node") && m.contains("role: pod") && m.contains("/metrics/cadvisor") && m.contains("prometheus_io_scrape"));
    // every namespaced object is in the monitoring namespace
    assert_eq!(m.matches(&format!("namespace: {PROM_NAMESPACE}")).count(), 6, "service account, binding subject, config map, claim, deployment, service");
    assert!(m.contains("app.kubernetes.io/managed-by: iyakki"));
}

#[test]
fn the_step_applies_waits_registers_the_dash_source_and_marks_it() {
    let mut f = Fake { k3s: true, ..Default::default() };
    run(&mut f).unwrap();
    assert_eq!(f.calls, ["apply prometheus", "rollout iyakki-monitoring/prometheus"]);
    assert_eq!(f.registered, ["prometheus-cluster=cluster://iyakki-monitoring/prometheus:9090"], "the stable address, not a cluster IP");
    assert_eq!(f.marks.len(), 1);
    assert_eq!(f.marks[0].0, Item::Prometheus);
    assert!(f.marks[0].1.contains("prometheus.iyakki-monitoring.svc:9090") && f.marks[0].1.contains("cluster://iyakki-monitoring/prometheus:9090"));
    assert!(f.lines.iter().any(|l| l.contains("Cluster Prometheus")));
    assert_eq!(crate::jobs::progress_hint(&f.lines.join("\n")), Some(100.0));
}

#[test]
fn a_refused_registration_still_installs_and_says_how_to_add_it() {
    let mut f = Fake { k3s: true, register_fails: true, ..Default::default() };
    run(&mut f).unwrap();
    assert!(f.lines.iter().any(|l| l.contains("could not add it to DASH (at most 50 sources)") && l.contains("cluster://iyakki-monitoring/prometheus:9090")));
    assert_eq!(f.marks.len(), 1, "the install itself counts");
}

#[test]
fn it_waits_for_k3s_and_stops_early_when_k3s_failed() {
    let mut f = Fake { k3s: false, ..Default::default() };
    let e = run(&mut f).unwrap_err();
    assert!(e.contains("not installed and Ready after 1 minutes"), "{e}");
    assert!(f.calls.is_empty() && f.marks.is_empty(), "nothing applied without a cluster");
    let mut f = Fake { k3s: false, deps: vec![(Item::K3s, DepState::Failed("download refused".into()))], ..Default::default() };
    let e = run(&mut f).unwrap_err();
    assert!(e.contains("k3s failed first") && e.contains("download refused"), "{e}");
    assert!(f.calls.is_empty());
}

#[test]
fn failures_have_a_plain_reason_and_leave_no_marker() {
    let mut f = Fake { k3s: true, apply_fails: true, ..Default::default() };
    assert!(run(&mut f).unwrap_err().contains("forbidden"));
    assert!(f.marks.is_empty() && f.registered.is_empty());
    let mut f = Fake { k3s: true, rollout_fails: true, ..Default::default() };
    let e = run(&mut f).unwrap_err();
    assert!(e.contains("ImagePullBackOff"), "{e}");
    assert!(f.marks.is_empty() && f.registered.is_empty(), "not registered when the pod never came up");
}

#[test]
fn the_dash_source_is_a_valid_prometheus_source() {
    let dir = std::env::temp_dir().join(format!("iyakki-qi-prom-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let s = crate::dash::store::Sources::open(&dir, "admin");
    s.save(prometheus_source()).unwrap();
    s.save(prometheus_source()).unwrap();
    let all = s.list();
    assert_eq!(all.len(), 1, "installing again replaces, never doubles");
    assert_eq!(all[0].id, PROM_SOURCE_ID);
    assert_eq!(all[0].value("url"), "cluster://iyakki-monitoring/prometheus:9090");
    assert!(all[0].secrets.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
