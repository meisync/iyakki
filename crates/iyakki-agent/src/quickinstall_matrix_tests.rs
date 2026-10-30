//! The Matrix server item with fakes only: no cluster, no network, no vault. Manifest text, the token by reference, the token
//! never in any output, registration never open, federation off, order, dependency on k3s, failure texts, running it twice, the image pin.

use super::*;

/// The token the fake cluster "generates"; it must appear in no line, manifest, mark or error.
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
    fn matrix_secret(&mut self) -> Result<bool, String> {
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
        if self.rollout_fails { Err("matrix did not become ready in 15 minutes: ImagePullBackOff".into()) } else { Ok(()) }
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
}

fn run(f: &mut Fake) -> Result<(), String> {
    run_step(Item::Matrix, f, Duration::from_secs(60), Duration::from_secs(5))
}

#[test]
fn the_item_is_a_cluster_item_that_needs_k3s_and_orders_after_it() {
    assert_eq!(Item::parse("matrix"), Some(Item::Matrix));
    assert_eq!(Item::Matrix.group(), Group::Cluster);
    assert_eq!(Item::Matrix.needs(), &[Item::K3s]);
    assert!(Item::Matrix.is_service() && !Item::Matrix.is_hardware() && Item::Matrix.runs_as_job());
    assert_eq!(order(Item::Matrix, &|_| false), vec![Item::K3s, Item::Matrix]);
    assert!(Item::ALL.contains(&Item::Matrix));
    let l = Item::Matrix.licence();
    assert!(l.contains("Apache-2.0") && l.contains("LICENSE") && l.contains("own pod") && l.contains("does not ship"), "{l}");
    assert!(!l.contains("AGPL"), "the chosen server is not AGPL");
}

#[test]
fn the_image_is_pinned_by_tag_and_digest_in_one_place_and_the_digest_is_a_real_looking_sha256() {
    assert!(matrix::IMAGE.starts_with("ghcr.io/matrix-construct/tuwunel:v1.9.4@sha256:"), "{}", matrix::IMAGE);
    let digest = matrix::IMAGE.split('@').nth(1).unwrap();
    assert_eq!(digest.len(), "sha256:".len() + 64, "a placeholder or a short digest fails here");
    let hex = &digest["sha256:".len()..];
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{digest}");
    assert!(hex.chars().collect::<std::collections::BTreeSet<_>>().len() > 6, "{digest} looks made up");
    let m = matrix::manifest();
    let images: Vec<&str> = m.lines().filter_map(|l| l.trim().strip_prefix("image: ")).collect();
    assert_eq!(images, [matrix::IMAGE], "one image, nothing else is pulled");
}

#[test]
fn the_manifest_is_unprivileged_limited_cluster_only_closed_to_registration_and_holds_no_secret() {
    let m = matrix::manifest();
    assert!(m.contains("runAsNonRoot: true") && m.contains("runAsUser: 10001") && m.contains("allowPrivilegeEscalation: false") && m.contains("drop: [ALL]"));
    assert!(m.contains("automountServiceAccountToken: false") && m.contains("type: RuntimeDefault"));
    assert!(!m.contains("privileged") && !m.contains("hostPath") && !m.contains("hostNetwork") && !m.contains("hostPID") && !m.contains("hostPort"));
    assert!(m.contains("type: ClusterIP") && !m.contains("NodePort") && !m.contains("LoadBalancer"));
    assert!(m.contains("port: 8008") && !m.contains("8448"), "the federation port is not exposed");
    assert!(m.contains("limits:") && m.contains("memory: 512Mi") && m.contains("requests:"));
    assert!(m.contains("claimName: matrix-data") && m.contains("storage: 5Gi") && m.contains("mountPath: /var/lib/tuwunel"));
    assert!(m.contains("readinessProbe:") && m.contains("/_matrix/client/versions"));
    assert!(m.contains("Recreate") && m.contains("replicas: 1"));
    // registration needs the token; open registration needs a flag that is never set; federation is off
    assert!(m.contains("secretKeyRef:") && m.contains(&format!("name: {}", matrix::SECRET)) && m.contains("key: TUWUNEL_REGISTRATION_TOKEN"));
    assert!(!m.to_ascii_lowercase().contains("open_registration") && !m.to_ascii_lowercase().contains("i_am_very_very_sure"));
    assert!(m.contains("name: TUWUNEL_ALLOW_FEDERATION\n              value: \"false\""), "{m}");
    assert!(!m.contains("kind: Secret") && !m.contains("stringData"), "{m}");
    assert!(!m.lines().any(|l| l.trim().starts_with("- name: TUWUNEL_REGISTRATION_TOKEN") && l.contains("value")), "no value here");
    assert_eq!(m.matches(&format!("namespace: {}", matrix::NS)).count(), 3, "its own namespace, not iyakki-data");
    assert!(!m.contains("iyakki-data"));
}

#[test]
fn the_step_makes_namespace_then_secret_then_objects_then_waits_and_marks() {
    let mut f = Fake { k3s: true, ..Default::default() };
    run(&mut f).unwrap();
    assert_eq!(f.calls, ["apply matrix-namespace", "secret", "apply matrix", "rollout iyakki-matrix/matrix"], "the Secret exists before the pod that reads it");
    assert_eq!(f.manifests[1], matrix::manifest());
    assert_eq!(f.afters, ["matrix-vault"]);
    assert_eq!(f.marks.len(), 1);
    assert_eq!(f.marks[0].0, Item::Matrix);
    assert!(f.marks[0].1.contains("cluster://iyakki-matrix/matrix:8008"), "the address for IYAKKI is in the result");
    assert!(f.lines.iter().any(|l| l.contains("Apache-2.0")), "the licence is in the job's first lines");
    assert!(f.lines.iter().any(|l| l.contains("Settings, Notifications, Matrix account") && l.contains("registration token")), "the next step is offered: {:?}", f.lines);
    assert_eq!(crate::jobs::progress_hint(&f.lines.join("\n")), Some(100.0));
}

#[test]
fn no_output_manifest_mark_or_error_ever_holds_the_token() {
    for secret_exists in [false, true] {
        let mut f = Fake { k3s: true, secret_exists, ..Default::default() };
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
fn running_again_keeps_the_secret_and_a_failed_start_leaves_no_mark() {
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
    assert!(f.marks.is_empty() && f.afters.is_empty());
}

#[test]
fn the_vault_handoff_names_the_token_entry_and_never_shows_the_token() {
    let read = || Ok(zeroize::Zeroizing::new(format!("{PW}\n")));
    let stored = std::cell::RefCell::new(String::new());
    let store = |t: &str| {
        *stored.borrow_mut() = t.to_string();
        Ok(())
    };
    let ok = matrix::handoff(&read, &store, &|_| {}, 2, Duration::ZERO).unwrap();
    assert_eq!(*stored.borrow(), PW, "the vault got the token");
    assert!(ok.contains("svc.matrix.registration-token") && ok.contains("cluster://iyakki-matrix/matrix:8008") && !ok.contains(PW), "{ok}");
    let never = |_: &str| Err::<(), String>("the vault is locked".into());
    let e = matrix::handoff(&read, &never, &|_| {}, 2, Duration::ZERO).unwrap_err();
    assert!(e.contains("svc.matrix.registration-token") && e.contains("kubectl -n iyakki-matrix get secret matrix-registration") && !e.contains(PW), "{e}");
}
