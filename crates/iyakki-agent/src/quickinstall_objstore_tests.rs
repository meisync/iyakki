//! The object storage items (SeaweedFS, RustFS) with fakes only: no cluster, no network, no vault. Item metadata, manifest text and
//! validity, the keys never in any output, the handoff, the typed record, remove with and without the data.

use super::*;
use objstore::{Engine, RemoveEnv, RUSTFS, SEAWEEDFS};

/// Keys the fake cluster "generates"; they must appear in no line, manifest, mark, error or record.
const AK: &str = "AKIAFAKE0000EXAMPLE1";
const SK: &str = "fakeSecretKeyForTestsOnly0123456789ABCDEF";

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
    execs: Vec<String>,
    image_id: Option<String>,
}

impl StepEnv for Fake {
    fn say(&mut self, line: &str) {
        self.lines.push(line.into());
    }
    fn dep_state(&mut self, _dep: Item) -> DepState {
        DepState::Ready
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
    fn objstore_secret(&mut self, e: &Engine) -> Result<bool, String> {
        self.calls.push(format!("secret {}", e.secret()));
        if self.secret_fails {
            return Err("kubectl create secret failed: forbidden".into());
        }
        let created = !self.secret_exists;
        self.secret_exists = true;
        Ok(created)
    }
    fn image_id(&mut self, _ns: &str, _deploy: &str) -> Option<String> {
        self.image_id.clone()
    }
    fn pod_exec(&mut self, ns: &str, deploy: &str, _c: &str, script: &str, _a: &str) -> Result<String, String> {
        self.execs.push(format!("{ns}/{deploy}: {script}"));
        Ok("created bucket iyakki".into())
    }
    fn rollout(&mut self, ns: &str, name: &str, _t: Duration) -> Result<(), String> {
        self.calls.push(format!("rollout {ns}/{name}"));
        if self.rollout_fails { Err("did not become ready in 15 minutes: ImagePullBackOff".into()) } else { Ok(()) }
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

#[test]
fn both_stores_are_cluster_items_in_one_section_that_say_pick_one_or_both() {
    for (it, id) in [(Item::SeaweedFs, "seaweedfs"), (Item::RustFs, "rustfs")] {
        assert_eq!(Item::parse(id), Some(it));
        assert_eq!(it.group(), Group::Cluster);
        assert_eq!(it.needs(), &[Item::K3s]);
        assert!(it.is_service() && !it.is_hardware() && it.runs_as_job() && it.cli_id().is_none());
        assert_eq!(order(it, &|_| false), vec![Item::K3s, it]);
        assert!(it.download_mb() > 0 && it.min_ram_mb() >= 1500);
        let l = it.licence();
        assert!(l.contains("Apache-2.0") && l.contains("own pod") && l.contains("does not ship"), "{l}");
        assert!(it.about().contains("Pick this one") && it.about().contains("both"), "{}", it.about());
    }
    assert!(items_of(Group::Cluster).contains(&Item::SeaweedFs) && items_of(Group::Cluster).contains(&Item::RustFs));
    assert!(Group::Cluster.note().contains("pick one or both"));
    assert!(Item::RustFs.about().contains("young") && Item::RustFs.about().contains("alpha"), "an honest maturity note");
    assert!(Item::SeaweedFs.about().contains("mature"));
    assert_eq!(cluster_object(Item::SeaweedFs), Some(("iyakki-objects", "seaweedfs")));
    assert_eq!(cluster_object(Item::RustFs), Some(("iyakki-objects", "rustfs")));
}

#[test]
fn the_image_is_pinned_by_tag_and_a_digest_is_never_invented() {
    for e in objstore::ENGINES {
        assert!(!e.tag.is_empty() && !e.tag.contains("latest"));
        // empty until a person fills it from the first real pull; when filled it must be a full sha256
        if !e.digest.is_empty() {
            let hex = e.digest.strip_prefix("sha256:").expect("sha256:");
            assert!(hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()), "{}", e.digest);
        }
        let m = objstore::manifest(e);
        let images: Vec<&str> = m.lines().filter_map(|l| l.trim().strip_prefix("image: ")).collect();
        assert_eq!(images, [e.image().as_str()], "one image, nothing else is pulled");
        assert!(images[0].starts_with(e.repo) && images[0].contains(e.tag));
    }
}

#[test]
fn the_manifests_are_unprivileged_limited_cluster_only_and_hold_no_secret() {
    for e in objstore::ENGINES {
        let m = objstore::manifest(e);
        assert!(m.contains("runAsNonRoot: true") && m.contains("runAsUser: 10001") && m.contains("allowPrivilegeEscalation: false") && m.contains("drop: [ALL]"), "{}", e.name);
        assert!(m.contains("automountServiceAccountToken: false") && m.contains("type: RuntimeDefault"));
        for bad in ["privileged", "hostPath", "hostNetwork", "hostPID", "hostPort", "NodePort", "LoadBalancer", "hostIPC", "kind: Secret", "stringData", "SYS_ADMIN"] {
            assert!(!m.contains(bad), "{}: {bad}", e.name);
        }
        assert!(m.contains("type: ClusterIP"));
        assert!(m.contains(&format!("containerPort: {}", e.port)) && m.contains(&format!("targetPort: {}", e.port)) && m.contains(&format!("      port: {}", e.port)));
        assert!(m.contains("limits:") && m.contains("memory: 1Gi") && m.contains("requests:"));
        assert!(m.contains(&format!("claimName: {}-data", e.name)) && m.contains("storage: 20Gi") && m.contains("mountPath: /data"));
        assert!(m.contains("readinessProbe:") && m.contains("Recreate") && m.contains("replicas: 1"));
        assert_eq!(m.matches("namespace: iyakki-objects").count(), 3);
        assert!(m.contains(&format!("name: {}", e.name)));
        // the Service publishes the S3 port only
        let svc = m.split("kind: Service").nth(1).unwrap();
        assert_eq!(svc.matches("- name:").count(), 1, "{svc}");
        // no key text and no env value that is a key
        assert!(!m.contains(AK) && !m.contains(SK));
    }
    let r = objstore::manifest(&RUSTFS);
    assert!(r.contains("secretKeyRef:") && r.contains("key: access-key") && r.contains("key: secret-key"));
    assert!(!r.lines().any(|l| l.trim().starts_with("- name: RUSTFS_ACCESS_KEY") && l.contains("value")));
    let s = objstore::manifest(&SEAWEEDFS);
    assert!(s.contains("secretName: seaweedfs-s3") && s.contains("readOnly: true") && s.contains("-s3.config=/etc/iyakki-s3/s3.json"));
    assert!(!s.contains("-s3.port=8333 -s3.port") && s.contains("\"-s3.port=8333\""));
}

#[test]
fn the_manifests_parse_as_yaml_documents_with_the_expected_kinds() {
    for e in objstore::ENGINES {
        let m = objstore::manifest(e);
        let kinds: Vec<&str> = m.split("---\n").map(|d| d.lines().find_map(|l| l.strip_prefix("kind: ")).unwrap_or("?")).collect();
        assert_eq!(kinds, ["PersistentVolumeClaim", "Deployment", "Service"]);
        assert!(!m.contains('\t'));
        for d in m.split("---\n") {
            assert!(d.starts_with("apiVersion:") && d.contains("metadata:"));
        }
    }
    assert!(objstore::namespace_manifest().contains("name: iyakki-objects"));
}

#[test]
fn the_secret_entries_hold_the_keys_and_seaweed_gets_its_identity_file() {
    let v = SEAWEEDFS.secret_values(AK, SK);
    let names: Vec<&str> = v.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(names, ["access-key", "secret-key", "s3.json"]);
    let doc: serde_json::Value = serde_json::from_str(&v[2].1).unwrap();
    assert_eq!(doc["identities"][0]["credentials"][0]["accessKey"], AK);
    assert_eq!(RUSTFS.secret_values(AK, SK).len(), 2);
    let (a, s) = objstore::new_keys().unwrap();
    assert_eq!((a.len(), s.len()), (20, 40));
    assert!(a.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
    let (a2, s2) = objstore::new_keys().unwrap();
    assert!(*a != *a2 && *s != *s2, "random");
}

#[test]
fn the_step_makes_namespace_then_secret_then_objects_then_waits_and_marks() {
    for e in objstore::ENGINES {
        let mut f = Fake { k3s: true, image_id: Some("docker.io/x/y@sha256:abc".into()), ..Default::default() };
        run_step(e.item, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
        assert_eq!(f.calls, ["apply objects-namespace".to_string(), format!("secret {}-s3", e.name), format!("apply {}", e.name), format!("rollout iyakki-objects/{}", e.name)], "the Secret exists before the pod that reads it");
        assert_eq!(f.manifests[1], objstore::manifest(e));
        assert_eq!(f.afters, [e.request]);
        assert_eq!(f.marks.len(), 1);
        assert!(f.marks[0].1.contains(&e.endpoint()) && f.marks[0].1.contains("sha256:abc"), "the image id the cluster resolved is recorded");
        assert!(f.lines.iter().any(|l| l.contains("Apache-2.0")), "the licence is in the job's first lines");
        assert_eq!(crate::jobs::progress_hint(&f.lines.join("\n")), Some(100.0));
        if e.item == Item::SeaweedFs {
            assert_eq!(f.execs.len(), 1);
            assert!(f.execs[0].contains("s3.bucket.create -name iyakki"));
        } else {
            assert!(f.execs.is_empty() && f.lines.iter().any(|l| l.contains("make the bucket")));
        }
    }
}

#[test]
fn no_output_manifest_mark_or_error_ever_holds_a_key() {
    for e in objstore::ENGINES {
        for secret_exists in [false, true] {
            let mut f = Fake { k3s: true, secret_exists, ..Default::default() };
            run_step(e.item, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
            let all = format!("{:?}{:?}{:?}{:?}{:?}", f.lines, f.manifests, f.marks, f.afters, f.execs);
            assert!(!all.contains(AK) && !all.contains(SK), "{all}");
        }
        for fail in 0..3 {
            let mut f = Fake { k3s: true, secret_fails: fail == 0, apply_fails: fail == 1, rollout_fails: fail == 2, ..Default::default() };
            let err = run_step(e.item, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap_err();
            assert!(!err.contains(AK) && !err.contains(SK) && !format!("{:?}", f.lines).contains(SK));
            assert!(f.marks.is_empty() && f.afters.is_empty(), "a failure leaves no marker");
        }
    }
}

#[test]
fn running_again_keeps_the_secret() {
    let mut f = Fake { k3s: true, ..Default::default() };
    run_step(Item::SeaweedFs, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    f.lines.clear();
    run_step(Item::SeaweedFs, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert!(f.lines.iter().any(|l| l.contains("is there already") && l.contains("kept")), "{:?}", f.lines);
}

#[test]
fn the_handoff_moves_both_keys_and_saves_a_record_that_holds_names_only() {
    let stored = std::cell::RefCell::new(Vec::<(String, String)>::new());
    let read = |k: &str| Ok(zeroize::Zeroizing::new(format!("{}\n", if k == "access-key" { AK } else { SK })));
    let store = |n: &str, v: &str| {
        stored.borrow_mut().push((n.into(), v.into()));
        Ok(())
    };
    let ok = objstore::finish(&SEAWEEDFS, &read, &store, &|_| {}, 2, Duration::ZERO).unwrap();
    assert!(ok.contains("svc.seaweedfs.access-key") && ok.contains("svc.seaweedfs.secret-key") && ok.contains("s3.seaweedfs") && !ok.contains(AK) && !ok.contains(SK), "{ok}");
    let s = stored.borrow();
    assert_eq!(s.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["svc.seaweedfs.access-key", "svc.seaweedfs.secret-key", "s3.seaweedfs"]);
    assert_eq!((s[0].1.as_str(), s[1].1.as_str()), (AK, SK), "the vault gets the keys");
    let rec = objstore::Connection::from_text(&s[2].1).unwrap();
    assert!(!s[2].1.contains(AK) && !s[2].1.contains(SK), "the record holds names, not keys");
    assert_eq!((rec.endpoint.as_str(), rec.bucket.as_str(), rec.region.as_str(), rec.path_style), ("http://seaweedfs.iyakki-objects.svc:8333", "iyakki", "us-east-1", true));
    assert_eq!((rec.access_key_vault.as_str(), rec.secret_key_vault.as_str()), ("svc.seaweedfs.access-key", "svc.seaweedfs.secret-key"));
    let never = |_: &str, _: &str| Err::<(), String>("the vault is locked".into());
    let e = objstore::finish(&RUSTFS, &read, &never, &|_| {}, 2, Duration::ZERO).unwrap_err();
    assert!(e.contains("kubectl -n iyakki-objects get secret rustfs-s3") && e.contains("svc.rustfs.secret-key") && !e.contains(AK) && !e.contains(SK), "{e}");
}

#[test]
fn the_connection_record_refuses_a_bad_endpoint_or_names() {
    let good = objstore::Connection::of(&RUSTFS);
    assert!(good.check().is_ok());
    for bad in [
        objstore::Connection { endpoint: "ftp://x".into(), ..good.clone() },
        objstore::Connection { endpoint: "http://user:pw@host:9000".into(), ..good.clone() },
        objstore::Connection { bucket: "a b".into(), ..good.clone() },
        objstore::Connection { secret_key_vault: "".into(), ..good.clone() },
    ] {
        assert!(bad.check().is_err(), "{bad:?}");
    }
    assert!(objstore::Connection::from_text("{\"engine\":1}").is_err());
}

#[derive(Default)]
struct Rm {
    kube: Vec<String>,
    vault: Vec<String>,
    unmarked: Vec<Item>,
}
impl RemoveEnv for Rm {
    fn kubectl(&mut self, args: &[&str]) -> Result<String, String> {
        self.kube.push(args.join(" "));
        Ok(String::new())
    }
    fn unmark(&mut self, item: Item) {
        self.unmarked.push(item);
    }
    fn vault_delete(&mut self, name: &str) -> Result<(), String> {
        self.vault.push(name.into());
        Ok(())
    }
}

#[test]
fn remove_keeps_the_data_and_the_keys_unless_asked_to_delete_them() {
    let mut r = Rm::default();
    let m = objstore::remove(&SEAWEEDFS, &mut r, false).unwrap();
    assert_eq!(r.kube, ["-n iyakki-objects delete deployment seaweedfs --ignore-not-found", "-n iyakki-objects delete service seaweedfs --ignore-not-found"]);
    assert!(r.vault.is_empty() && r.unmarked == [Item::SeaweedFs]);
    assert!(m.contains("kept") && m.contains("seaweedfs-data"), "{m}");
    let mut r = Rm::default();
    let m = objstore::remove(&RUSTFS, &mut r, true).unwrap();
    assert!(r.kube.iter().any(|k| k.contains("delete pvc rustfs-data")) && r.kube.iter().any(|k| k.contains("delete secret rustfs-s3")));
    assert_eq!(r.vault, ["svc.rustfs.access-key", "svc.rustfs.secret-key", "s3.rustfs"]);
    assert!(m.contains("with its data"));
    // the other store is not touched
    assert!(r.kube.iter().all(|k| !k.contains("seaweedfs")));
}

#[test]
fn two_stores_do_not_share_names_so_both_can_run() {
    assert_ne!(SEAWEEDFS.secret(), RUSTFS.secret());
    assert_ne!(SEAWEEDFS.claim(), RUSTFS.claim());
    assert_ne!(SEAWEEDFS.record_name(), RUSTFS.record_name());
    assert_ne!(SEAWEEDFS.request, RUSTFS.request);
}

#[test]
fn the_pending_vault_entry_names_the_secret_key_and_the_store() {
    let p = pending_of(Item::RustFs, "admin");
    assert_eq!((p.namespace.as_str(), p.secret.as_str(), p.key.as_str(), p.vault.as_str()), ("iyakki-objects", "rustfs-s3", "secret-key", "svc.rustfs.secret-key"));
    assert!(p.what.contains("RustFS"));
}
