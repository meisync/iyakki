//! The graph database item with fakes only: no cluster, no network, no vault. Manifest text, the secret by reference, the password
//! never in any output, order, dependency on k3s, failure texts, running it twice.

use super::*;

/// The password the fake cluster "generates"; it must appear in no line, manifest, mark or error.
const PW: &str = "Zx9Qm4Tr7Lp2Kd8Vb3Nc6Wh1";

#[derive(Default)]
struct Fake {
    lines: Vec<String>,
    calls: Vec<String>,
    manifests: Vec<String>,
    k3s: bool,
    secret_exists: bool,
    secret_fails: bool,
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
    fn graph_secret(&mut self) -> Result<bool, String> {
        self.calls.push("secret".into());
        if self.secret_fails {
            return Err("kubectl create secret failed: the server could not find the requested resource".into());
        }
        let created = !self.secret_exists;
        self.secret_exists = true;
        Ok(created)
    }
    fn rollout(&mut self, ns: &str, name: &str, _t: Duration) -> Result<(), String> {
        self.calls.push(format!("rollout {ns}/{name}"));
        if self.rollout_fails { Err("graphdb did not become ready in 15 minutes: ImagePullBackOff".into()) } else { Ok(()) }
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
    run_step(Item::GraphDb, f, Duration::from_secs(60), Duration::from_secs(5))
}

#[test]
fn the_item_is_a_cluster_item_that_needs_k3s_and_orders_after_it() {
    assert_eq!(Item::parse("graph-db"), Some(Item::GraphDb));
    assert_eq!(Item::GraphDb.title(), "Graph database (for AI)");
    assert_eq!(Item::GraphDb.group(), Group::Cluster);
    assert_eq!(Item::GraphDb.needs(), &[Item::K3s]);
    assert!(Item::GraphDb.is_service() && !Item::GraphDb.is_hardware() && Item::GraphDb.runs_as_job());
    assert_eq!(order(Item::GraphDb, &|_| false), vec![Item::K3s, Item::GraphDb]);
    assert_eq!(order(Item::GraphDb, &|i| i == Item::K3s), vec![Item::GraphDb]);
    assert_eq!(label(Item::GraphDb, 2, 2), "Install Graph database (for AI) (2/2, after k3s)");
    assert!(Item::ALL.contains(&Item::GraphDb));
}

#[test]
fn the_licence_text_says_plainly_what_it_is() {
    let l = Item::GraphDb.licence();
    assert!(l.contains("GPL-3.0") && l.contains("copyleft") && l.contains("own pod") && l.contains("does not ship"), "{l}");
    assert!(Item::GraphDb.min_ram_mb() >= 3000, "the Setup list warns small machines");
}

#[test]
fn the_image_is_pinned_by_tag_and_digest_in_one_place() {
    assert!(GRAPH_IMAGE.starts_with("docker.io/library/neo4j:2026.10.1-community@sha256:"), "{GRAPH_IMAGE}");
    let digest = GRAPH_IMAGE.split('@').nth(1).unwrap();
    assert_eq!(digest.len(), "sha256:".len() + 64);
    assert!(digest["sha256:".len()..].chars().all(|c| c.is_ascii_hexdigit()));
    let m = graphdb_manifest();
    let images: Vec<&str> = m.lines().filter_map(|l| l.trim().strip_prefix("image: ")).collect();
    assert_eq!(images, [GRAPH_IMAGE], "one image, nothing else is pulled");
}

#[test]
fn the_manifest_is_unprivileged_limited_cluster_only_and_holds_no_secret() {
    let m = graphdb_manifest();
    assert!(m.contains("runAsNonRoot: true") && m.contains("runAsUser: 7474") && m.contains("allowPrivilegeEscalation: false") && m.contains("readOnlyRootFilesystem: true") && m.contains("drop: [ALL]"));
    assert!(m.contains("automountServiceAccountToken: false") && m.contains("type: RuntimeDefault"));
    assert!(!m.contains("privileged") && !m.contains("hostPath") && !m.contains("hostNetwork") && !m.contains("hostPID") && !m.contains("hostPort"));
    assert!(m.contains("type: ClusterIP") && !m.contains("NodePort") && !m.contains("LoadBalancer"));
    assert!(m.contains("limits:") && m.contains("memory: 1Gi") && m.contains("requests:") && m.contains("memory: 640Mi"));
    assert!(m.contains("claimName: graphdb-data") && m.contains("storage: 5Gi"));
    assert!(m.contains("Recreate") && m.contains("replicas: 1"));
    // the password is taken by reference from the Secret; no value, no stringData, no Secret object in this text
    assert!(m.contains("secretKeyRef:") && m.contains(&format!("name: {GRAPH_SECRET}")) && m.contains("key: NEO4J_AUTH"));
    assert!(!m.contains("kind: Secret") && !m.contains("stringData") && !m.contains("password"), "{m}");
    assert!(!m.lines().any(|l| l.trim().starts_with("- name: NEO4J_AUTH") && l.contains("value")), "NEO4J_AUTH has no value here");
    assert_eq!(m.matches(&format!("namespace: {NAMESPACE}")).count(), 3, "claim, deployment, service and nothing else");
    assert!(m.contains("app.kubernetes.io/managed-by: iyakki"));
    // the writable places of a read-only root file system are the volume and emptyDirs only
    for p in ["/data", "/logs", "/tmp", "/var/lib/neo4j/conf", "/var/lib/neo4j/run"] {
        assert!(m.contains(&format!("mountPath: {p}")), "{p}");
    }
}

#[test]
fn the_step_makes_namespace_then_secret_then_objects_then_waits_registers_and_marks() {
    let mut f = Fake { k3s: true, ..Default::default() };
    run(&mut f).unwrap();
    assert_eq!(f.calls, ["apply data-namespace", "secret", "apply graphdb", "rollout iyakki-data/graphdb"], "the Secret exists before the pod that reads it");
    assert_eq!(f.manifests[0], data_namespace_manifest());
    assert_eq!(f.manifests[1], graphdb_manifest());
    assert_eq!(f.registered.len(), 1);
    let s = &f.registered[0];
    assert_eq!((s.id.as_str(), s.kind.as_str(), s.value("url")), (GRAPH_SOURCE_ID, "neo4j", "cluster://iyakki-data/graphdb:7474"), "the stable address, not a cluster IP");
    assert_eq!(s.secrets.get("password").map(String::as_str), Some("uvault:svc.graphdb"), "a vault reference, never the value");
    assert!(s.values.keys().all(|k| k != "password"));
    assert_eq!(f.afters, ["graph-vault"], "the shell is asked to put the password in the vault");
    assert_eq!(f.marks.len(), 1);
    assert_eq!(f.marks[0].0, Item::GraphDb);
    assert!(f.marks[0].1.contains("graphdb.iyakki-data.svc:7474") && f.marks[0].1.contains(GRAPH_DASH_URL));
    assert_eq!(crate::jobs::progress_hint(&f.lines.join("\n")), Some(100.0));
    assert!(f.lines.iter().any(|l| l.contains("GPL-3.0")), "the licence is in the job's first lines");
}

#[test]
fn the_source_is_valid_for_dash_and_installing_twice_replaces_it() {
    let dir = std::env::temp_dir().join(format!("iyakki-qi-graph-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let s = crate::dash::store::Sources::open(&dir, "admin");
    s.save(graphdb_source()).unwrap();
    s.save(graphdb_source()).unwrap();
    let all = s.list();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].kind, "neo4j");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_output_manifest_mark_or_error_ever_holds_the_password() {
    // the fake cluster knows PW; nothing the step sees or says may contain it
    for (secret_exists, register_fails) in [(false, false), (true, false), (false, true)] {
        let mut f = Fake { k3s: true, secret_exists, register_fails, ..Default::default() };
        run(&mut f).unwrap();
        let everything = format!("{:?}{:?}{:?}{:?}", f.lines, f.manifests, f.marks, f.afters);
        assert!(!everything.contains(PW), "{everything}");
        assert!(!everything.contains(&format!("{GRAPH_USER}/{PW}")), "no NEO4J_AUTH value (user/password) either");
    }
    for fail in 0..3 {
        let mut f = Fake { k3s: true, secret_fails: fail == 0, apply_fails: fail == 1, rollout_fails: fail == 2, ..Default::default() };
        let e = run(&mut f).unwrap_err();
        assert!(!e.contains(PW) && !format!("{:?}", f.lines).contains(PW));
    }
    // the generator itself: 24 letters and digits, different every time, never the fake's
    let a = new_graph_password().unwrap();
    let b = new_graph_password().unwrap();
    assert_eq!(a.len(), 24);
    assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
    assert_ne!(*a, *b);
}

#[test]
fn running_again_keeps_the_secret_and_changes_nothing_else() {
    let mut f = Fake { k3s: true, ..Default::default() };
    run(&mut f).unwrap();
    assert!(f.lines.iter().any(|l| l.contains("made the administrator password")));
    let first_manifests = f.manifests.clone();
    f.lines.clear();
    f.calls.clear();
    f.manifests.clear();
    f.registered.clear();
    run(&mut f).unwrap();
    assert!(f.lines.iter().any(|l| l.contains("is there already") && l.contains("kept")), "{:?}", f.lines);
    assert_eq!(f.manifests, first_manifests, "the same objects: applying again changes nothing");
    assert_eq!(f.registered.len(), 1);
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
fn failures_have_a_plain_reason_and_leave_no_marker_or_source() {
    let mut f = Fake { k3s: true, apply_fails: true, ..Default::default() };
    assert!(run(&mut f).unwrap_err().contains("forbidden"));
    assert!(f.marks.is_empty() && f.registered.is_empty() && f.afters.is_empty());
    let mut f = Fake { k3s: true, secret_fails: true, ..Default::default() };
    let e = run(&mut f).unwrap_err();
    assert!(e.contains("create secret failed"), "{e}");
    assert!(f.calls == ["apply data-namespace", "secret"], "the pod is not applied without its Secret: {:?}", f.calls);
    assert!(f.marks.is_empty() && f.registered.is_empty());
    let mut f = Fake { k3s: true, rollout_fails: true, ..Default::default() };
    let e = run(&mut f).unwrap_err();
    assert!(e.contains("ImagePullBackOff"), "{e}");
    assert!(f.marks.is_empty() && f.registered.is_empty() && f.afters.is_empty(), "not registered when the pod never came up");
}

#[test]
fn a_refused_registration_still_installs_and_says_how_to_add_it() {
    let mut f = Fake { k3s: true, register_fails: true, ..Default::default() };
    run(&mut f).unwrap();
    assert!(f.lines.iter().any(|l| l.contains("could not add it to DASH (at most 40 sources)") && l.contains(GRAPH_DASH_URL)));
    assert_eq!(f.marks.len(), 1, "the install itself counts");
}

#[test]
fn the_vault_handoff_retries_a_locked_vault_and_never_shows_the_password() {
    use std::cell::{Cell, RefCell};
    let stored = RefCell::new(Vec::<String>::new());
    let tries = Cell::new(0);
    let read = || Ok(zeroize::Zeroizing::new(format!("{PW}\n")));
    let store = |pw: &str| {
        tries.set(tries.get() + 1);
        if tries.get() < 3 {
            return Err("the vault is locked".to_string());
        }
        stored.borrow_mut().push(pw.to_string());
        Ok(())
    };
    let ok = graph_vault_handoff(&read, &store, &|_| {}, 5, Duration::from_secs(3)).unwrap();
    assert_eq!(*stored.borrow(), [PW], "trimmed, stored once, as the vault entry");
    assert!(ok.contains(GRAPH_VAULT_NAME) && !ok.contains(PW));
    // never unlocked: a plain failure that says what to do, without the password
    let never = |_: &str| Err::<(), String>("the vault is locked".into());
    let e = graph_vault_handoff(&read, &never, &|_| {}, 3, Duration::from_secs(3)).unwrap_err();
    assert!(e.contains("vault is locked") && e.contains("svc.graphdb") && e.contains("kubectl -n iyakki-data get secret graphdb-auth") && !e.contains(PW), "{e}");
    // the cluster does not answer
    let down = || Err::<zeroize::Zeroizing<String>, String>("connection refused".into());
    let e = graph_vault_handoff(&down, &store, &|_| {}, 2, Duration::from_secs(3)).unwrap_err();
    assert!(e.contains("could not read the password from the cluster"), "{e}");
    let empty = || Ok(zeroize::Zeroizing::new(String::new()));
    assert!(graph_vault_handoff(&empty, &store, &|_| {}, 1, Duration::ZERO).unwrap_err().contains("holds no password"));
}
