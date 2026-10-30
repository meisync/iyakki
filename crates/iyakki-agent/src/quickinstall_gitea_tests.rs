//! The Gitea item with fakes only: no cluster, no network, no vault. Manifest reuse, the pinned image, the loopback address, no password
//! or token in any output, the vault handoff, the Git connection added once, the IDE offer, failure texts, the k3s dependency.

use std::cell::RefCell;
use std::collections::BTreeMap;

use std::sync::Arc;

use super::gitea::*;
use super::gitea::{presence, NAMESPACE};
use super::*;
use crate::gitconn::{Conns, Kind, Method};
use crate::gitkeys::MemStore;
use crate::platform::gitea::{self as pg, Gitea, LOOPBACK_OPTION};
use crate::platform::installed::{self, ClusterRef, Installed};
use crate::platform::{BuildCtx, Options, ServiceDef};

/// What the fake pod answers for the token command; it must appear in no line, manifest, mark, error, record or connection file.
const TOKEN: &str = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0";
/// What the fake cluster holds as the administrator password (made by `generated_secret`).
const PW: &str = "Zx9Qm4Tr7Lp2Kd8Vb3Nc6Wh1";

#[derive(Default)]
struct Fake {
    lines: Vec<String>,
    calls: Vec<String>,
    manifests: Vec<(String, String)>,
    k3s: bool,
    /// (ns/name, key) -> value: the fake cluster's Secrets.
    secrets: BTreeMap<String, String>,
    records: Vec<Installed>,
    busy_ports: Vec<u16>,
    health: Vec<Result<u16, String>>,
    apply_fails: bool,
    rollout_fails: bool,
    admin_error: Option<String>,
    token_output: Option<String>,
    marks: Vec<(Item, String)>,
    afters: Vec<String>,
    deps: Vec<(Item, DepState)>,
    live: Option<LiveGitea>,
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
        self.manifests.push((name.into(), manifest.into()));
        if self.apply_fails { Err("kubectl apply failed: forbidden".into()) } else { Ok(format!("{name} configured")) }
    }
    fn rollout(&mut self, ns: &str, name: &str, _t: Duration) -> Result<(), String> {
        self.calls.push(format!("rollout {ns}/{name}"));
        if self.rollout_fails { Err("gitea did not become ready in 15 minutes: ImagePullBackOff".into()) } else { Ok(()) }
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
    fn cluster_ref(&mut self) -> Result<ClusterRef, String> {
        Ok(ClusterRef { name: "k3s".into(), kubeconfig: "/data/k3s-dev/kubeconfig".into(), context: "default".into(), server: "https://127.0.0.1:6443".into(), source: crate::devcluster::Source::K3s })
    }
    fn service_records(&mut self) -> Vec<Installed> {
        self.records.clone()
    }
    fn save_service(&mut self, rec: Installed) -> Result<(), String> {
        self.calls.push("record".into());
        self.records.retain(|r| r.id != rec.id);
        self.records.push(rec);
        Ok(())
    }
    fn port_free(&mut self, port: u16) -> bool {
        !self.busy_ports.contains(&port)
    }
    fn live_gitea(&mut self) -> Option<LiveGitea> {
        self.live.clone()
    }
    fn http_status(&mut self, url: &str) -> Result<u16, String> {
        self.calls.push(format!("get {url}"));
        if self.health.is_empty() { Ok(200) } else { self.health.remove(0) }
    }
    fn generated_secret(&mut self, ns: &str, name: &str, key: &str, instance: &str) -> Result<bool, String> {
        self.calls.push(format!("secret {name} for {instance}"));
        let k = format!("{ns}/{name}/{key}");
        if self.secrets.contains_key(&k) {
            return Ok(false);
        }
        self.secrets.insert(k, PW.into());
        Ok(true)
    }
    fn put_secret(&mut self, ns: &str, name: &str, key: &str, value: &str, _instance: &str) -> Result<(), String> {
        self.calls.push(format!("put {name}"));
        self.secrets.insert(format!("{ns}/{name}/{key}"), value.into());
        Ok(())
    }
    fn pod_exec(&mut self, ns: &str, deploy: &str, container: &str, script: &str, _arg0: &str) -> Result<String, String> {
        self.calls.push(format!("exec {ns}/{deploy}/{container}"));
        if script == pg::ADMIN_CREATE_SCRIPT {
            return match &self.admin_error {
                Some(e) => Err(e.clone()),
                None => Ok("New user 'iyakki-admin' has been successfully created!".into()),
            };
        }
        assert_eq!(script, pg::TOKEN_SCRIPT);
        Ok(self.token_output.clone().unwrap_or_else(|| format!("{TOKEN}\n")))
    }
}

fn run(f: &mut Fake) -> Result<(), String> {
    run_step(Item::Gitea, f, Duration::from_secs(60), Duration::from_secs(5))
}

fn up() -> Fake {
    Fake { k3s: true, ..Default::default() }
}

fn everything(f: &Fake) -> String {
    format!("{:?}{:?}{:?}{:?}{:?}", f.lines, f.manifests, f.marks, f.afters, f.records)
}

fn temp(tag: &str) -> PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 3];
    iyakki_sec::random_fill(&mut r).unwrap();
    let d = Path::new(&base).join(format!("qi-gitea-{tag}-{:02x}{:02x}{:02x}", r[0], r[1], r[2]));
    std::fs::create_dir_all(&d).unwrap();
    d
}

// ------------------------------------------------------------------------------------------------ the item

#[test]
fn the_item_is_a_cluster_item_that_needs_k3s_and_orders_after_it() {
    assert_eq!(Item::parse("gitea"), Some(Item::Gitea));
    assert_eq!(Item::Gitea.title(), "Gitea (versioning for your work)");
    assert_eq!(Item::Gitea.group(), Group::Cluster);
    assert_eq!(Item::Gitea.needs(), &[Item::K3s]);
    assert!(Item::Gitea.is_service() && !Item::Gitea.is_hardware() && Item::Gitea.runs_as_job());
    assert_eq!(order(Item::Gitea, &|_| false), vec![Item::K3s, Item::Gitea]);
    assert_eq!(order(Item::Gitea, &|i| i == Item::K3s), vec![Item::Gitea]);
    assert!(Item::ALL.contains(&Item::Gitea));
    let ids: std::collections::BTreeSet<&str> = Item::ALL.iter().map(|i| i.id()).collect();
    assert_eq!(ids.len(), Item::ALL.len(), "ids are unique");
}

#[test]
fn the_licence_text_says_plainly_what_it_is() {
    let l = Item::Gitea.licence();
    assert!(l.contains("MIT") && l.contains("own pod") && l.contains("does not ship") && l.contains("1.24.5"), "{l}");
    assert!(Item::Gitea.about().contains("http://127.0.0.1:3300") && Item::Gitea.about().contains("Local Gitea"));
    assert!(Item::Gitea.min_ram_mb() >= 1000);
}

#[test]
fn the_image_is_pinned_by_tag_and_digest_and_is_a_version_platform_services_knows() {
    assert!(IMAGE.starts_with("docker.io/gitea/gitea:1.24.5-rootless@sha256:"), "{IMAGE}");
    let digest = IMAGE.split('@').nth(1).unwrap();
    assert_eq!(digest.len(), "sha256:".len() + 64);
    assert!(digest["sha256:".len()..].chars().all(|c| c.is_ascii_hexdigit()));
    assert!(Gitea.versions().contains(&VERSION.to_string()), "the pinned version is one of Platform services' versions");
    assert!(Gitea.images(VERSION)[0].image.contains(&format!("{VERSION}-rootless")));
}

// ------------------------------------------------------------------------------------------------ the objects

#[test]
fn the_objects_are_platform_services_own_with_a_loopback_port_and_nothing_else() {
    let (built, opts) = build(3300, "gitea-iyakki-gitea").unwrap();
    // the same text Platform services builds for the same context: nothing is copied
    let ctx = BuildCtx { namespace: NAMESPACE.into(), opts: opts.clone(), version: VERSION.into(), images: vec![IMAGE.into()], storage_class: None, host: "localhost".into(), http_port: 0, ssh_port: 0, vault_prefix: "svc.gitea-iyakki-gitea".into(), instance: "gitea-iyakki-gitea".into() };
    assert_eq!(built.manifest, Gitea.build(&ctx).unwrap().manifest);
    let m = &built.manifest;
    assert_eq!(m.matches(&format!("image: {IMAGE}")).count(), 1, "one image, by tag and digest");
    assert!(m.contains("runAsNonRoot: true") && m.contains("runAsUser: 1000") && m.contains("allowPrivilegeEscalation: false") && m.contains("drop:\n            - ALL") && m.contains("type: RuntimeDefault"));
    assert!(m.contains("automountServiceAccountToken: false") && m.contains("limits:") && m.contains("claimName: gitea-data"));
    assert!(!m.contains("privileged") && !m.contains("hostPath") && !m.contains("hostNetwork") && !m.contains("hostPID"));
    // the Services stay inside the cluster; the only host port is on the loopback address
    assert!(!m.contains("NodePort") && !m.contains("LoadBalancer") && !m.contains("kind: Ingress"));
    assert_eq!(m.matches("type: ClusterIP").count(), 2);
    assert_eq!(m.matches("hostPort:").count(), 1);
    assert!(m.contains("hostPort: 3300\n          hostIP: 127.0.0.1"), "{m}");
    assert!(m.contains("value: \"http://127.0.0.1:3300/\""), "Gitea builds its clone addresses from the address this machine uses");
    // the password only by reference
    assert!(m.contains("secretKeyRef:") && m.contains("name: gitea-admin") && m.contains("key: password"));
    assert!(!m.contains("kind: Secret") && !m.contains("stringData"));
    // SSH is not published, so no SSH clone address is offered
    assert!(built.urls.iter().all(|u| u.kind != "clone-ssh"));
    assert!(built.urls.iter().any(|u| u.kind == "web" && u.url == "http://127.0.0.1:3300"));
    assert!(built.notes.iter().any(|n| n.contains("127.0.0.1:3300") && n.contains("no other computer can")));
    assert_eq!(opts.get("exposure"), "ClusterIP");
    assert_eq!(opts.get("namespace"), NAMESPACE);
}

#[test]
fn the_manifest_passes_the_manifest_check_and_gets_its_namespace() {
    let (built, _) = build(3300, "gitea-iyakki-gitea").unwrap();
    let lint = crate::deploylint::lint(&built.manifest, NAMESPACE, &|_| false);
    assert!(lint.ok(), "{}", lint.report());
    let m = with_namespace(&built.manifest, NAMESPACE);
    let docs = built.manifest.split("---\n").filter(|d| !d.trim().is_empty()).count();
    assert_eq!(m.matches(&format!("\n  namespace: {NAMESPACE}\n")).count(), docs, "every object, once: {m}");
    assert!(m.contains("kind: PersistentVolumeClaim") && m.contains("kind: Deployment") && m.contains("kind: Service"));
    assert!(!m.contains("      namespace:"), "the pod template's metadata is left alone");
    let ns = namespace_manifest();
    assert!(ns.contains("name: iyakki-gitea") && ns.contains("app.kubernetes.io/managed-by: iyakki") && ns.contains("iyakki.dev/service: gitea"));
}

#[test]
fn the_loopback_port_is_validated_by_platform_services_rules() {
    assert!(options(3300).is_ok());
    assert!(options(80).unwrap_err().contains("1024"));
    let mut o = Options::default();
    o.set("exposure", "NodePort");
    o.set(LOOPBACK_OPTION, "3300");
    assert!(crate::platform::validate_options(&Gitea, &o).unwrap_err().contains("ClusterIP"));
    // without the option Platform services' Gitea is exactly what it was
    let mut plain = Options::default();
    plain.set("exposure", "ClusterIP");
    let plain = crate::platform::validate_options(&Gitea, &plain).unwrap();
    assert!(Gitea::loopback_of(&plain).is_none());
}

// ------------------------------------------------------------------------------------------------ the job

#[test]
fn the_step_goes_namespace_secret_objects_wait_health_admin_token_record_in_that_order() {
    let mut f = up();
    run(&mut f).unwrap();
    assert_eq!(
        f.calls,
        [
            "apply gitea-namespace",
            "secret gitea-admin for gitea-iyakki-gitea",
            "apply gitea",
            "rollout iyakki-gitea/gitea",
            "get http://127.0.0.1:3300/api/healthz",
            "exec iyakki-gitea/gitea/gitea",
            "exec iyakki-gitea/gitea/gitea",
            "put gitea-iyakki-token",
            "record",
        ],
        "the Secret exists before the pod that reads it"
    );
    assert_eq!(f.manifests[0].1, namespace_manifest());
    assert_eq!(f.manifests[1].1, with_namespace(&build(3300, "gitea-iyakki-gitea").unwrap().0.manifest, NAMESPACE));
    assert_eq!(f.afters, [REQUEST], "the shell is asked to do the vault handoff");
    assert_eq!(f.marks.len(), 1);
    assert_eq!(f.marks[0].0, Item::Gitea);
    let d = &f.marks[0].1;
    assert!(d.contains("http://127.0.0.1:3300") && d.contains("http://127.0.0.1:3300/iyakki-admin/<repo>.git") && d.contains("this machine only"), "{d}");
    assert!(f.lines.last().unwrap().contains(d.as_str()), "the result line states the addresses");
    assert!(f.lines.iter().any(|l| l.contains("MIT")), "the licence is in the job's first lines");
    assert_eq!(crate::jobs::progress_hint(&f.lines.join("\n")), Some(100.0));
}

#[test]
fn the_record_is_platform_services_own_so_the_assistant_and_the_connect_flows_see_it() {
    let mut f = up();
    run(&mut f).unwrap();
    let r = &f.records[0];
    assert_eq!((r.id.as_str(), r.service.as_str(), r.kind.as_str(), r.namespace.as_str(), r.version.as_str()), ("gitea-iyakki-gitea", "gitea", "git", "iyakki-gitea", "1.24.5"));
    assert!(ours(r) && !r.connected && r.created_namespace);
    assert_eq!(r.web_url(), Some("http://127.0.0.1:3300"));
    assert_eq!(r.cluster.as_ref().map(|c| c.server.as_str()), Some("https://127.0.0.1:6443"));
    // the token's vault name is the one Platform services' tools look for
    assert_eq!(token_vault(r), "svc.gitea-iyakki-gitea.token");
    assert_eq!(admin_vault(r), "svc.gitea-iyakki-gitea.admin-password");
    // and the GIT tile's cluster list sees it as usable (no "only reachable inside the cluster")
    let dir = temp("rec");
    installed::save(&dir, "alice", &f.records).unwrap();
    let mut with_token = f.records[0].clone();
    with_token.vault.push("svc.gitea-iyakki-gitea.token".into());
    installed::save(&dir, "alice", &[with_token]).unwrap();
    let l = crate::gitconn::cluster_gitea(&dir, "alice");
    assert_eq!((l[0].url.as_str(), l[0].problem.as_str()), ("http://127.0.0.1:3300", ""), "{l:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_output_manifest_mark_record_or_error_ever_holds_the_password_or_the_token() {
    let mut f = up();
    run(&mut f).unwrap();
    let all = everything(&f);
    assert!(!all.contains(PW) && !all.contains(TOKEN), "{all}");
    // the only place a value goes is a Secret
    assert_eq!(f.secrets.get("iyakki-gitea/gitea-iyakki-token/token").map(String::as_str), Some(TOKEN));
    assert_eq!(f.secrets.get("iyakki-gitea/gitea-admin/password").map(String::as_str), Some(PW));
    for fail in 0..5 {
        let mut f = up();
        match fail {
            0 => f.apply_fails = true,
            1 => f.rollout_fails = true,
            2 => f.admin_error = Some(format!("boom {PW}")),
            3 => f.token_output = Some("not a token at all".into()),
            _ => f.health = vec![Err("refused".into()); 12],
        }
        let e = run(&mut f).unwrap_err();
        assert!(!e.contains(TOKEN) && !everything(&f).contains(TOKEN), "{e}");
        if fail != 2 {
            assert!(!e.contains(PW));
        }
    }
}

#[test]
fn running_again_keeps_the_secret_the_port_and_the_token_and_changes_nothing_else() {
    let mut f = up();
    run(&mut f).unwrap();
    // the shell's handoff noted the token in the record
    f.records[0].vault.push("svc.gitea-iyakki-gitea.token".into());
    let first = f.manifests.clone();
    f.lines.clear();
    f.calls.clear();
    f.manifests.clear();
    f.busy_ports = vec![3300];
    f.admin_error = Some("Error: user already exists [name: iyakki-admin]".into());
    run(&mut f).unwrap();
    assert!(f.lines.iter().any(|l| l.contains("is there already") && l.contains("kept")), "{:?}", f.lines);
    assert!(f.lines.iter().any(|l| l.contains("exists") && l.contains("administrator")), "{:?}", f.lines);
    assert!(f.lines.iter().any(|l| l.contains("API token is in your vault already")));
    assert_eq!(f.manifests, first, "the same objects, the same port even though our own pod holds it");
    assert!(!f.calls.iter().any(|c| c.starts_with("put ")), "no second token: {:?}", f.calls);
    assert_eq!(f.records.len(), 1);
}

#[test]
fn the_first_free_loopback_port_is_used_and_none_free_is_a_plain_failure() {
    let mut f = up();
    f.busy_ports = vec![3300, 3301];
    run(&mut f).unwrap();
    assert_eq!(f.records[0].web_url(), Some("http://127.0.0.1:3302"));
    assert!(f.marks[0].1.contains("127.0.0.1:3302"));
    let mut f = up();
    f.busy_ports = (PORT_FIRST..=PORT_LAST).collect();
    let e = run(&mut f).unwrap_err();
    assert!(e.contains("3300 to 3309") && e.contains("Install again"), "{e}");
    assert!(f.calls.is_empty(), "nothing is applied without an address");
}

#[test]
fn it_waits_for_k3s_and_stops_early_when_k3s_failed() {
    let mut f = Fake { k3s: false, ..Default::default() };
    let e = run(&mut f).unwrap_err();
    assert!(e.contains("not installed and Ready after 1 minutes"), "{e}");
    assert!(f.calls.is_empty() && f.marks.is_empty());
    let mut f = Fake { k3s: false, deps: vec![(Item::K3s, DepState::Failed("download refused".into()))], ..Default::default() };
    let e = run(&mut f).unwrap_err();
    assert!(e.contains("k3s failed first") && e.contains("download refused"), "{e}");
    assert!(f.calls.is_empty());
}

#[test]
fn failures_have_a_plain_reason_and_leave_no_marker_record_or_request() {
    let mut f = up();
    f.apply_fails = true;
    assert!(run(&mut f).unwrap_err().contains("forbidden"));
    assert!(f.marks.is_empty() && f.records.is_empty() && f.afters.is_empty());
    let mut f = up();
    f.rollout_fails = true;
    assert!(run(&mut f).unwrap_err().contains("ImagePullBackOff"));
    assert!(f.marks.is_empty() && f.records.is_empty() && f.afters.is_empty());
    let mut f = up();
    f.health = vec![Err("connection refused".into()); 12];
    let e = run(&mut f).unwrap_err();
    assert!(e.contains("http://127.0.0.1:3300 does not answer from this machine") && e.contains("port 3300"), "{e}");
    assert!(f.marks.is_empty() && f.records.is_empty());
    let mut f = up();
    f.admin_error = Some("database is locked".into());
    let e = run(&mut f).unwrap_err();
    assert!(e.contains("creating the administrator failed") && e.contains("database is locked"), "{e}");
    assert!(f.records.is_empty() && f.afters.is_empty());
    let mut f = up();
    f.token_output = Some("garbage".into());
    assert!(run(&mut f).unwrap_err().contains("not with a token"));
    assert!(f.marks.is_empty() && f.afters.is_empty());
    // a slow start is waited for
    let mut f = up();
    f.health = vec![Err("refused".into()), Ok(503), Ok(200)];
    run(&mut f).unwrap();
}

#[test]
fn a_gitea_from_platform_services_is_used_when_reachable_and_explained_when_not() {
    let theirs = |exposure: &str, url: &str| {
        let mut o = Options::default();
        o.set("exposure", exposure);
        Installed { id: "gitea-iyakki-gitea".into(), service: "gitea".into(), kind: "git".into(), namespace: "iyakki-gitea".into(), options: o, urls: vec![crate::platform::UrlInfo { label: "Gitea".into(), url: url.into(), kind: "web".into() }], vault: vec!["svc.gitea-iyakki-gitea.token".into()], ..Default::default() }
    };
    let mut f = up();
    f.records = vec![theirs("NodePort", "http://192.168.1.5:30300")];
    run(&mut f).unwrap();
    assert!(f.calls.is_empty(), "nothing in the cluster is touched: {:?}", f.calls);
    assert_eq!(f.afters, [REQUEST]);
    assert!(f.marks[0].1.contains("http://192.168.1.5:30300"));
    let mut f = up();
    f.records = vec![theirs("ClusterIP", "http://gitea-http.iyakki-gitea.svc.cluster.local:3000")];
    let e = run(&mut f).unwrap_err();
    assert!(e.contains("only reachable inside the cluster") && e.contains("Platform services") && e.contains("Install again"), "{e}");
    assert!(f.calls.is_empty() && f.marks.is_empty());
    // a connected (not deployed) Gitea does not stand in the way
    let mut c = theirs("NodePort", "https://git.example.org");
    c.connected = true;
    c.id = "gitea-existing-aaaaaa".into();
    let mut f = up();
    f.records = vec![c];
    run(&mut f).unwrap();
    assert_eq!(f.records.len(), 2);
}

// ------------------------------------------------------------------------------------------------ the shell's half

struct Cluster {
    secrets: RefCell<BTreeMap<String, String>>,
    dropped: RefCell<Vec<String>>,
}

fn setup(tag: &str) -> (PathBuf, Arc<MemStore>, Conns, Cluster) {
    let dir = temp(tag);
    let store = Arc::new(MemStore::default());
    let conns = Conns::new(&dir.join("keys"), store.clone(), Arc::new(|_, _| {}));
    let mut f = up();
    run(&mut f).unwrap();
    installed::save(&dir, "alice", &f.records).unwrap();
    let c = Cluster { secrets: RefCell::new(f.secrets.clone()), dropped: RefCell::new(vec![]) };
    (dir, store, conns, c)
}

fn handoff_with<'a>(c: &'a Cluster, store: &'a Arc<MemStore>, locked_for: &'a std::cell::Cell<u32>) -> impl Fn() -> Result<String, String> + 'a {
    move || {
        let read = |secret: &str, key: &str| -> Result<Option<zeroize::Zeroizing<String>>, String> { Ok(c.secrets.borrow().get(&format!("iyakki-gitea/{secret}/{key}")).map(|v| zeroize::Zeroizing::new(format!("{v}\n")))) };
        let put = |n: &str, v: &str| {
            if locked_for.get() > 0 {
                locked_for.set(locked_for.get() - 1);
                return Err("the vault is locked".to_string());
            }
            crate::gitkeys::SecretStore::set(&**store, n, v)
        };
        let has = |n: &str| crate::gitkeys::SecretStore::names(&**store).unwrap().iter().any(|x| x == n);
        let drop_secret = |n: &str| {
            c.dropped.borrow_mut().push(n.to_string());
            c.secrets.borrow_mut().retain(|k, _| !k.contains(n));
            Ok(())
        };
        let h = Handoff { read: &read, store: &put, has: &has, drop_secret: &drop_secret, sleep: &|_| {}, tries: 5, wait: Duration::from_secs(3) };
        handoff(&h, "svc.gitea-iyakki-gitea.admin-password", "svc.gitea-iyakki-gitea.token")
    }
}

#[test]
fn the_handoff_puts_both_values_in_the_vault_retries_a_locked_vault_and_drops_the_token_secret() {
    let (dir, store, _conns, c) = setup("hand");
    let locked = std::cell::Cell::new(2);
    let go = handoff_with(&c, &store, &locked);
    let line = go().unwrap();
    assert_eq!(crate::gitkeys::SecretStore::get(&*store, "svc.gitea-iyakki-gitea.token").unwrap().as_str(), TOKEN, "trimmed, stored under the name Platform services uses");
    assert_eq!(crate::gitkeys::SecretStore::get(&*store, "svc.gitea-iyakki-gitea.admin-password").unwrap().as_str(), PW);
    assert!(line.contains("svc.gitea-iyakki-gitea.token") && !line.contains(TOKEN) && !line.contains(PW), "{line}");
    assert_eq!(*c.dropped.borrow(), ["gitea-iyakki-token"], "the cluster keeps no second copy of the token");
    // running it again (the Secret is gone, the vault has the value) is fine and keeps the vault entry
    let line2 = go().unwrap();
    assert!(!line2.contains(TOKEN));
    assert_eq!(crate::gitkeys::SecretStore::get(&*store, "svc.gitea-iyakki-gitea.token").unwrap().as_str(), TOKEN);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_vault_that_never_unlocks_is_a_plain_failure_without_values() {
    let (dir, store, _conns, c) = setup("locked");
    let locked = std::cell::Cell::new(1000);
    let e = handoff_with(&c, &store, &locked)().unwrap_err();
    assert!(e.contains("vault is locked") && e.contains("press Install on Gitea again") && !e.contains(TOKEN) && !e.contains(PW), "{e}");
    assert!(c.dropped.borrow().is_empty(), "the token Secret stays until the vault has the token");
    // the cluster does not answer
    let read = |_: &str, _: &str| Err::<Option<zeroize::Zeroizing<String>>, String>("connection refused".into());
    let nope = |_: &str, _: &str| Ok(());
    let h = Handoff { read: &read, store: &nope, has: &|_| false, drop_secret: &|_| Ok(()), sleep: &|_| {}, tries: 2, wait: Duration::ZERO };
    assert!(handoff(&h, "a", "b").unwrap_err().contains("could not read the administrator password from the cluster"));
    // no Secret and nothing in the vault
    let none = |_: &str, _: &str| Ok(None);
    let h = Handoff { read: &none, store: &nope, has: &|_| false, drop_secret: &|_| Ok(()), sleep: &|_| {}, tries: 1, wait: Duration::ZERO };
    assert!(handoff(&h, "a", "b").unwrap_err().contains("holds no administrator password and your vault has none"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_git_connection_is_added_once_with_the_vault_token_and_no_copy() {
    let (dir, store, conns, c) = setup("conn");
    let locked = std::cell::Cell::new(0);
    let read = |secret: &str, key: &str| -> Result<Option<zeroize::Zeroizing<String>>, String> { Ok(c.secrets.borrow().get(&format!("iyakki-gitea/{secret}/{key}")).map(|v| zeroize::Zeroizing::new(v.clone()))) };
    let put = |n: &str, v: &str| {
        let _ = &locked;
        crate::gitkeys::SecretStore::set(&*store, n, v)
    };
    let has = |n: &str| crate::gitkeys::SecretStore::names(&*store).unwrap().iter().any(|x| x == n);
    let h = Handoff { read: &read, store: &put, has: &has, drop_secret: &|_| Ok(()), sleep: &|_| {}, tries: 3, wait: Duration::ZERO };
    let line = complete(&h, &conns, &dir, "alice").unwrap();
    assert!(line.contains("added the Git connection \"Local Gitea\" at http://127.0.0.1:3300"), "{line}");
    assert!(!line.contains(TOKEN) && !line.contains(PW));
    let list = conns.list();
    assert_eq!(list.len(), 1);
    let l = &list[0];
    assert_eq!((l.name.as_str(), l.url.as_str(), l.token_vault.as_str(), l.cluster.as_str()), ("Local Gitea", "http://127.0.0.1:3300", "svc.gitea-iyakki-gitea.token", "gitea-iyakki-gitea"));
    assert_eq!((l.kind, l.method), (Kind::Gitea, Method::Token));
    assert_eq!(conns.token(l).unwrap().as_str(), TOKEN, "the connection reads the platform's own vault entry");
    assert!(!crate::gitkeys::SecretStore::names(&*store).unwrap().iter().any(|n| n.starts_with("git.conn.")), "the token is not copied");
    assert_eq!(l.placement(), crate::gitconn::Placement::Cluster);
    // the record notes the vault names, so the next run keeps the token
    let rec = installed::find(&dir, "alice", "gitea-iyakki-gitea").unwrap();
    assert!(rec.vault.contains(&"svc.gitea-iyakki-gitea.token".to_string()) && rec.vault.contains(&"svc.gitea-iyakki-gitea.admin-password".to_string()));
    // twice: still one connection, "kept"
    let again = complete(&h, &conns, &dir, "alice").unwrap();
    assert!(again.contains("kept the Git connection"), "{again}");
    assert_eq!(conns.list().len(), 1);
    // no file in the data folder holds a value
    for p in walk(&dir) {
        let t = std::fs::read_to_string(&p).unwrap_or_default();
        assert!(!t.contains(TOKEN) && !t.contains(PW), "{}", p.display());
    }
    // the address moved (another port after a reinstall): the connection follows
    let mut rec = installed::find(&dir, "alice", "gitea-iyakki-gitea").unwrap();
    rec.urls.iter_mut().for_each(|u| u.url = u.url.replace(":3300", ":3301"));
    installed::upsert(&dir, "alice", rec).unwrap();
    complete(&h, &conns, &dir, "alice").unwrap();
    assert_eq!(conns.list().len(), 1);
    assert_eq!(conns.list()[0].url, "http://127.0.0.1:3301");
    let _ = std::fs::remove_dir_all(&dir);
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
    }
    out
}

#[test]
fn a_connection_the_person_named_the_same_is_not_overwritten() {
    let (dir, store, conns, _c) = setup("name");
    conns.add(crate::gitconn::Connection { name: "Local Gitea".into(), kind: Kind::Gitea, url: "https://git.example.org".into(), ..Default::default() }, Some("tok_FAKEFAKEFAKEFAKEFAKE")).unwrap();
    crate::gitkeys::SecretStore::set(&*store, "svc.gitea-iyakki-gitea.token", TOKEN).unwrap();
    let rec = installed::find(&dir, "alice", "gitea-iyakki-gitea").unwrap();
    let (c, made) = register_connection(&conns, &rec).unwrap();
    assert!(made && c.name == "Local Gitea (IYAKKI)");
    assert_eq!(conns.list().len(), 2);
    assert_eq!(conns.list()[0].url, "https://git.example.org");
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------------------------------------ the IDE's offer and the Setup row

#[test]
fn the_ide_offers_local_gitea_only_when_the_project_has_no_remote() {
    let (dir, _store, conns, _c) = setup("offer");
    // no remote, nothing installed: the plain message names Store -> Setup
    match offer(false, &[]) {
        Offer::Install(m) => assert!(m.contains("Store") && m.contains("Setup") && m.contains("Gitea (versioning for your work)"), "{m}"),
        o => panic!("{o:?}"),
    }
    // a remote: nothing is offered, whatever is installed
    assert_eq!(offer(true, &[]), Offer::None);
    let rec = installed::find(&dir, "alice", "gitea-iyakki-gitea").unwrap();
    let (c, _) = register_connection(&conns, &rec).unwrap();
    assert_eq!(offer(true, &conns.list()), Offer::None);
    assert_eq!(offer(false, &conns.list()), Offer::Ready(c.id.clone()));
    // an ordinary Gitea connection of the person is not the local one
    let other = crate::gitconn::Connection { name: "Home Gitea".into(), kind: Kind::Gitea, url: "https://git.example.org".into(), ..Default::default() };
    assert!(matches!(offer(false, &[other]), Offer::Install(_)));
    assert!(OFFER_LABEL.contains("Local Gitea") && OFFER_LABEL.contains("version control"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_setup_row_is_installed_only_while_the_record_is_there() {
    let dir = temp("presence");
    assert_eq!(presence(&dir), Presence::Missing);
    let mut f = up();
    run(&mut f).unwrap();
    mark_done(&dir, Item::Gitea, &f.marks[0].1).unwrap();
    assert_eq!(presence(&dir), Presence::Missing, "a marker alone is not installed: the record was removed");
    installed::save(&dir, "alice", &f.records).unwrap();
    match presence(&dir) {
        Presence::Installed(d) => assert!(d.contains("http://127.0.0.1:3300") && d.contains("this machine only"), "{d}"),
        p => panic!("{p:?}"),
    }
    installed::save(&dir, "alice", &[]).unwrap();
    assert_eq!(presence(&dir), Presence::Missing);
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------------------------------------ adoption (N2)

#[test]
fn a_platform_gitea_open_on_the_network_without_a_record_is_adopted_and_nothing_in_the_cluster_changes() {
    let mut f = up();
    f.live = Some(LiveGitea { exposure: "NodePort".into(), http_node_port: 30300, ssh_node_port: 30222 });
    run(&mut f).unwrap();
    // no apply, no namespace, no secret generated: the cluster is left as it is
    assert!(f.calls.iter().all(|c| !c.starts_with("apply") && !c.starts_with("secret ") && !c.starts_with("rollout")), "{:?}", f.calls);
    assert!(f.calls.iter().any(|c| c == "record") && f.afters == vec![REQUEST.to_string()]);
    let rec = &f.records[0];
    assert_eq!((rec.service.as_str(), rec.namespace.as_str(), rec.options.get("exposure"), rec.options.get("http_node_port")), ("gitea", NAMESPACE, "NodePort", "30300"));
    assert!(!ours(rec) && rec.web_url() == Some("http://localhost:30300"), "{:?}", rec.urls);
    assert!(f.secrets.contains_key(&format!("{NAMESPACE}/{TOKEN_SECRET}/token")));
    assert!(!everything(&f).contains(TOKEN));
    // Platform services sees it as the Gitea that is installed and keeps its exposure on a redeploy
    let facts = crate::platform::ClusterFacts::default();
    assert_eq!(crate::platform::engine::kept_gitea_exposure(&f.records, &facts, NAMESPACE).map(|x| x.0), Some("NodePort".to_string()));
    assert!(matches!(presence_of(&f.records), Presence::Installed(_)));
    // a second run uses the record: nothing new is made
    let n = f.records.len();
    f.calls.clear();
    run(&mut f).unwrap();
    assert_eq!(f.records.len(), n);
    assert!(f.calls.iter().all(|c| !c.starts_with("apply")));
}

fn presence_of(records: &[Installed]) -> Presence {
    let d = temp("presence");
    crate::platform::installed::save(&d, "alice", records).unwrap();
    presence(&d)
}

#[test]
fn a_clusterip_gitea_without_a_record_is_adopted_by_the_normal_idempotent_run_and_is_then_ours() {
    let mut f = up();
    f.live = Some(LiveGitea { exposure: "ClusterIP".into(), http_node_port: 0, ssh_node_port: 0 });
    // the existing volume's Secret is there: its password is kept
    f.secrets.insert(format!("{NAMESPACE}/gitea-admin/password"), PW.into());
    run(&mut f).unwrap();
    assert!(f.lines.iter().any(|l| l.contains("is in the cluster already without a record")), "{:?}", f.lines);
    assert!(f.lines.iter().any(|l| l.contains("is there already")), "password kept: {:?}", f.lines);
    assert_eq!(f.records.len(), 1);
    assert!(ours(&f.records[0]) && f.records[0].options.get("loopback_port") == "3300");
    // and Platform services' default deploy plans the same shape (ClusterIP, 127.0.0.1:3300), so the two installers agree
    let (_, o) = build(3300, "x").unwrap();
    assert_eq!((o.get("exposure"), o.get("loopback_port")), ("ClusterIP", "3300"));
}

#[test]
fn a_quick_install_record_is_a_platform_services_install_and_a_platform_one_is_reused() {
    // Quick install's record is found by Platform services (installed list) and is kept narrow on a redeploy
    let mut f = up();
    run(&mut f).unwrap();
    let rec = f.records[0].clone();
    assert_eq!(rec.kind, "git");
    let facts = crate::platform::ClusterFacts::default();
    let kept = crate::platform::engine::kept_gitea_exposure(&f.records, &facts, NAMESPACE).unwrap();
    assert_eq!(kept.0, "ClusterIP");
    assert_eq!(kept.1, vec![("loopback_port".to_string(), "3300".to_string())]);
    // a Platform services record (NodePort, with a vault token) is reused by Quick install without touching the cluster
    let mut g = up();
    let mut theirs = rec;
    theirs.options.set("exposure", "NodePort");
    theirs.options.set("loopback_port", "");
    theirs.urls = vec![crate::platform::UrlInfo { label: "Gitea".into(), url: "http://localhost:30300".into(), kind: "web".into() }];
    g.records = vec![theirs];
    run(&mut g).unwrap();
    assert!(g.calls.iter().all(|c| !c.starts_with("apply")), "{:?}", g.calls);
}
