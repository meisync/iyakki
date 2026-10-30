//! The OpenSearch item with fakes only: no cluster, no network. Manifest text, the pinned image, the licence wording, the DASH source,
//! order, dependency on k3s, failure texts.

use super::*;

#[derive(Default)]
struct Fake {
    lines: Vec<String>,
    calls: Vec<String>,
    manifests: Vec<String>,
    k3s: bool,
    apply_fails: bool,
    rollout_fails: bool,
    register_fails: bool,
    registered: Vec<crate::dash::connector::SourceConfig>,
    marks: Vec<(Item, String)>,
    afters: Vec<String>,
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
        self.manifests.push(manifest.into());
        if self.apply_fails { Err("kubectl apply failed: forbidden".into()) } else { Ok(format!("{name} configured")) }
    }
    fn rollout(&mut self, ns: &str, name: &str, _t: Duration) -> Result<(), String> {
        self.calls.push(format!("rollout {ns}/{name}"));
        if self.rollout_fails { Err("opensearch did not become ready in 15 minutes: ImagePullBackOff".into()) } else { Ok(()) }
    }
    fn sleep(&mut self, _d: Duration) {}
    fn mark(&mut self, item: Item, detail: &str) -> Result<(), String> {
        self.marks.push((item, detail.into()));
        Ok(())
    }
    fn request_after(&mut self, what: &str) -> Result<(), String> {
        self.afters.push(what.into());
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
            return Err("at most 40 sources".into());
        }
        self.registered.push(cfg);
        Ok(())
    }
}

fn run(f: &mut Fake) -> Result<(), String> {
    run_step(Item::OpenSearch, f, Duration::from_secs(60), Duration::from_secs(5))
}

#[test]
fn the_item_is_a_cluster_item_that_needs_k3s_and_orders_after_it() {
    assert_eq!(Item::parse("opensearch"), Some(Item::OpenSearch));
    assert_eq!(Item::OpenSearch.group(), Group::Cluster);
    assert_eq!(Item::OpenSearch.needs(), &[Item::K3s]);
    assert!(Item::OpenSearch.is_service() && !Item::OpenSearch.is_hardware() && Item::OpenSearch.runs_as_job());
    assert_eq!(order(Item::OpenSearch, &|_| false), vec![Item::K3s, Item::OpenSearch]);
    assert!(Item::ALL.contains(&Item::OpenSearch));
    let l = Item::OpenSearch.licence();
    assert!(l.contains("Apache-2.0") && l.contains("own pod") && l.contains("does not ship"), "{l}");
}

#[test]
fn elasticsearch_says_it_is_source_available_and_never_shipped() {
    let l = Item::Elasticsearch.licence();
    assert!(l.contains("Elastic License 2.0") && l.contains("SSPL") && l.contains("not open source") && l.contains("never shipped"), "{l}");
    assert!(Item::Elasticsearch.about().contains("OpenSearch"), "points to the open-source choice");
}

#[test]
fn the_image_is_pinned_by_tag_and_digest_in_one_place() {
    assert!(opensearch::IMAGE.starts_with("docker.io/opensearchproject/opensearch:3.9.0@sha256:"), "{}", opensearch::IMAGE);
    let digest = opensearch::IMAGE.split('@').nth(1).unwrap();
    assert_eq!(digest.len(), "sha256:".len() + 64);
    assert!(digest["sha256:".len()..].chars().all(|c| c.is_ascii_hexdigit()));
    let m = opensearch::manifest();
    let images: Vec<&str> = m.lines().filter_map(|l| l.trim().strip_prefix("image: ")).collect();
    assert_eq!(images, [opensearch::IMAGE], "one image, nothing else is pulled");
}

#[test]
fn the_manifest_is_unprivileged_limited_and_cluster_only() {
    let m = opensearch::manifest();
    assert!(m.contains("runAsNonRoot: true") && m.contains("runAsUser: 1000") && m.contains("allowPrivilegeEscalation: false") && m.contains("drop: [ALL]"));
    assert!(m.contains("automountServiceAccountToken: false") && m.contains("type: RuntimeDefault"));
    assert!(!m.contains("privileged") && !m.contains("hostPath") && !m.contains("hostNetwork") && !m.contains("hostPort"));
    assert!(m.contains("type: ClusterIP") && !m.contains("NodePort") && !m.contains("LoadBalancer"));
    assert!(m.contains("limits:") && m.contains("memory: 1536Mi") && m.contains("storage: 5Gi") && m.contains("claimName: opensearch-data"));
    assert!(m.contains("discovery.type") && m.contains("single-node") && m.contains("readinessProbe:"));
    assert!(!m.contains("kind: Secret") && !m.contains("stringData"));
    assert_eq!(m.matches(&format!("namespace: {NAMESPACE}")).count(), 3);
}

#[test]
fn the_source_uses_the_elasticsearch_connector_at_the_cluster_address() {
    let s = opensearch::source();
    assert_eq!((s.kind.as_str(), s.id.as_str(), s.name.as_str()), ("elasticsearch", "opensearch-cluster", "Cluster OpenSearch"));
    assert_eq!(s.values.get("url").map(String::as_str), Some("cluster://iyakki-data/opensearch:9200"));
    assert!(s.secrets.is_empty());
}

#[test]
fn the_step_applies_waits_registers_and_marks_and_failures_say_why() {
    let mut f = Fake { k3s: true, ..Default::default() };
    run(&mut f).unwrap();
    assert_eq!(f.calls, ["apply data-namespace", "apply opensearch", "rollout iyakki-data/opensearch"]);
    assert_eq!(f.registered.len(), 1);
    assert_eq!(f.marks.len(), 1);
    assert!(f.marks[0].1.contains("opensearch.iyakki-data.svc:9200"), "{:?}", f.marks);
    // the DASH source is optional: its failure is said, the install still counts
    let mut f = Fake { k3s: true, register_fails: true, ..Default::default() };
    run(&mut f).unwrap();
    assert!(f.lines.iter().any(|l| l.contains("could not add it to DASH")) && f.marks.len() == 1);
    // a failed rollout is not marked as installed
    let mut f = Fake { k3s: true, rollout_fails: true, ..Default::default() };
    assert!(run(&mut f).unwrap_err().contains("did not become ready"));
    assert!(f.marks.is_empty());
}
