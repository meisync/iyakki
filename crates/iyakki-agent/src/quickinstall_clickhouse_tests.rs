//! The ClickHouse item with fakes only: no cluster, no network, no vault. Manifest text, the secret by reference, the password
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
    fn clickhouse_secret(&mut self) -> Result<bool, String> {
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
        if self.rollout_fails { Err("clickhouse did not become ready in 15 minutes: ImagePullBackOff".into()) } else { Ok(()) }
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
    run_step(Item::ClickHouse, f, Duration::from_secs(60), Duration::from_secs(5))
}

#[test]
fn the_item_is_a_cluster_item_that_needs_k3s_and_orders_after_it() {
    assert_eq!(Item::parse("clickhouse"), Some(Item::ClickHouse));
    assert_eq!(Item::ClickHouse.group(), Group::Cluster);
    assert_eq!(Item::ClickHouse.needs(), &[Item::K3s]);
    assert!(Item::ClickHouse.is_service() && !Item::ClickHouse.is_hardware() && Item::ClickHouse.runs_as_job());
    assert_eq!(order(Item::ClickHouse, &|_| false), vec![Item::K3s, Item::ClickHouse]);
    assert!(Item::ALL.contains(&Item::ClickHouse));
    let l = Item::ClickHouse.licence();
    assert!(l.contains("Apache-2.0") && l.contains("own pod") && l.contains("does not ship"), "{l}");
}

#[test]
fn the_image_is_pinned_by_tag_and_digest_in_one_place() {
    assert!(clickhouse::IMAGE.starts_with("docker.io/clickhouse/clickhouse-server:26.8.9.10@sha256:"), "{}", clickhouse::IMAGE);
    let digest = clickhouse::IMAGE.split('@').nth(1).unwrap();
    assert_eq!(digest.len(), "sha256:".len() + 64);
    assert!(digest["sha256:".len()..].chars().all(|c| c.is_ascii_hexdigit()));
    let m = clickhouse::manifest();
    let images: Vec<&str> = m.lines().filter_map(|l| l.trim().strip_prefix("image: ")).collect();
    assert_eq!(images, [clickhouse::IMAGE], "one image, nothing else is pulled");
}

#[test]
fn the_manifest_is_unprivileged_limited_cluster_only_and_holds_no_secret() {
    let m = clickhouse::manifest();
    assert!(m.contains("runAsNonRoot: true") && m.contains("runAsUser: 101") && m.contains("allowPrivilegeEscalation: false") && m.contains("drop: [ALL]"));
    assert!(m.contains("automountServiceAccountToken: false") && m.contains("type: RuntimeDefault"));
    assert!(!m.contains("privileged") && !m.contains("hostPath") && !m.contains("hostNetwork") && !m.contains("hostPID") && !m.contains("hostPort"));
    assert!(m.contains("type: ClusterIP") && !m.contains("NodePort") && !m.contains("LoadBalancer"));
    assert!(m.contains("port: 8123") && m.contains("port: 9000"));
    assert!(m.contains("limits:") && m.contains("memory: 1Gi") && m.contains("requests:"));
    assert!(m.contains("claimName: clickhouse-data") && m.contains("storage: 5Gi") && m.contains("mountPath: /var/lib/clickhouse"));
    assert!(m.contains("readinessProbe:") && m.contains("path: /ping"));
    assert!(m.contains("Recreate") && m.contains("replicas: 1"));
    assert!(m.contains("secretKeyRef:") && m.contains(&format!("name: {}", clickhouse::SECRET)) && m.contains("key: CLICKHOUSE_PASSWORD"));
    assert!(!m.contains("kind: Secret") && !m.contains("stringData"), "{m}");
    assert!(!m.lines().any(|l| l.trim().starts_with("- name: CLICKHOUSE_PASSWORD") && l.contains("value")), "no value here");
    assert_eq!(m.matches(&format!("namespace: {NAMESPACE}")).count(), 3);
}

#[test]
fn the_step_makes_namespace_then_secret_then_objects_then_waits_registers_and_marks() {
    let mut f = Fake { k3s: true, ..Default::default() };
    run(&mut f).unwrap();
    assert_eq!(f.calls, ["apply data-namespace", "secret", "apply clickhouse", "rollout iyakki-data/clickhouse"], "the Secret exists before the pod that reads it");
    assert_eq!(f.manifests[1], clickhouse::manifest());
    assert_eq!(f.registered.len(), 1);
    let s = &f.registered[0];
    assert_eq!((s.id.as_str(), s.kind.as_str(), s.value("url")), ("clickhouse-cluster", "clickhouse", "cluster://iyakki-data/clickhouse:8123"), "the stable address, not a cluster IP");
    assert_eq!(s.secrets.get("password").map(String::as_str), Some("uvault:svc.clickhouse"), "a vault reference, never the value");
    assert!(s.values.keys().all(|k| k != "password"));
    assert_eq!(f.afters, ["clickhouse-vault"]);
    assert_eq!(f.marks.len(), 1);
    assert_eq!(f.marks[0].0, Item::ClickHouse);
    assert!(f.lines.iter().any(|l| l.contains("Apache-2.0")), "the licence is in the job's first lines");
    assert_eq!(crate::jobs::progress_hint(&f.lines.join("\n")), Some(100.0));
}

#[test]
fn no_output_manifest_mark_or_error_ever_holds_the_password() {
    for (secret_exists, register_fails) in [(false, false), (true, false), (false, true)] {
        let mut f = Fake { k3s: true, secret_exists, register_fails, ..Default::default() };
        run(&mut f).unwrap();
        let everything = format!("{:?}{:?}{:?}{:?}", f.lines, f.manifests, f.marks, f.afters);
        assert!(!everything.contains(PW), "{everything}");
    }
    for fail in 0..3 {
        let mut f = Fake { k3s: true, secret_fails: fail == 0, apply_fails: fail == 1, rollout_fails: fail == 2, ..Default::default() };
        let e = run(&mut f).unwrap_err();
        assert!(!e.contains(PW) && !format!("{:?}", f.lines).contains(PW));
    }
}

#[test]
fn running_again_keeps_the_secret_and_failures_leave_nothing_behind() {
    let mut f = Fake { k3s: true, ..Default::default() };
    run(&mut f).unwrap();
    let first = f.manifests.clone();
    f.lines.clear();
    f.manifests.clear();
    run(&mut f).unwrap();
    assert!(f.lines.iter().any(|l| l.contains("is there already") && l.contains("kept")), "{:?}", f.lines);
    assert_eq!(f.manifests, first);
    let mut f = Fake { k3s: true, rollout_fails: true, ..Default::default() };
    assert!(run(&mut f).unwrap_err().contains("ImagePullBackOff"));
    assert!(f.marks.is_empty() && f.registered.is_empty() && f.afters.is_empty());
    let mut f = Fake { k3s: true, register_fails: true, ..Default::default() };
    run(&mut f).unwrap();
    assert!(f.lines.iter().any(|l| l.contains("could not add it to DASH") && l.contains(clickhouse::DASH_URL)));
    assert_eq!(f.marks.len(), 1);
}

#[test]
fn the_source_replaces_itself_and_removing_the_item_removes_it() {
    let dir = std::env::temp_dir().join(format!("iyakki-qi-clickhouse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sys = crate::dash::store::Sources::open(&dir, "system");
    sys.save(clickhouse::source()).unwrap();
    sys.save(clickhouse::source()).unwrap();
    assert_eq!(sys.list().len(), 1);
    // a hand-added source is left alone
    let mut mine = crate::dash::connector::SourceConfig { id: "clickhouse".into(), kind: "clickhouse".into(), name: "ClickHouse".into(), ..Default::default() };
    mine.values.insert("url".into(), "http://localhost:8123".into());
    crate::dash::store::Sources::open(&dir, "admin").save(mine).unwrap();
    assert_eq!(clickhouse::remove_source(&dir, "admin"), 1);
    assert!(sys.get("clickhouse-cluster").is_none());
    assert!(crate::dash::store::Sources::open(&dir, "admin").get("clickhouse").is_some());
    assert_eq!(clickhouse::remove_source(&dir, "admin"), 0, "nothing left to remove");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_vault_handoff_names_clickhouse_and_never_shows_the_password() {
    let read = || Ok(zeroize::Zeroizing::new(format!("{PW}\n")));
    let store = |_: &str| Ok(());
    let ok = vault_handoff(&clickhouse::handoff(), &read, &store, &|_| {}, 2, Duration::ZERO).unwrap();
    assert!(ok.contains("svc.clickhouse") && ok.contains("Cluster ClickHouse") && !ok.contains(PW), "{ok}");
    let never = |_: &str| Err::<(), String>("the vault is locked".into());
    let e = vault_handoff(&clickhouse::handoff(), &read, &never, &|_| {}, 2, Duration::ZERO).unwrap_err();
    assert!(e.contains("svc.clickhouse") && e.contains("kubectl -n iyakki-data get secret clickhouse-auth") && !e.contains(PW), "{e}");
}
