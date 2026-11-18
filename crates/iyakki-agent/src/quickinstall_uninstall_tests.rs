//! Uninstall of every Quick install item with fakes only: no cluster, no network, no real files outside a temp folder.

use super::*;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-qiu-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ctx(data: &Path) -> PlanCtx {
    PlanCtx { data: data.to_path_buf(), gitea: Some(("gitea-iyakki-gitea".into(), vec!["svc.gitea-iyakki-gitea.admin-password".into(), "svc.gitea-iyakki-gitea.token".into()])), model_hosts: vec!["mac-studio".into()], cli_warning: None, volatile: false }
}

#[derive(Default)]
struct Fake {
    kube: Vec<String>,
    vault: Vec<String>,
    sources: Vec<String>,
    hooks: Vec<Hook>,
    audits: Vec<String>,
    fail_on: Option<String>,
}

impl UninstallEnv for Fake {
    fn kubectl(&mut self, args: &[&str]) -> Result<String, String> {
        let line = args.join(" ");
        if let Some(f) = &self.fail_on {
            if line.contains(f.as_str()) {
                return Err("the cluster did not answer".into());
            }
        }
        self.kube.push(line.clone());
        Ok(if line.contains("{.spec.volumeName}") { "pvc-1234".into() } else if line.contains("spec.local.path") { String::new() } else { String::new() })
    }
    fn vault_delete(&mut self, name: &str) -> Result<(), String> {
        self.vault.push(name.into());
        Ok(())
    }
    fn source_remove(&mut self, id: &str) -> Result<(), String> {
        self.sources.push(id.into());
        Ok(())
    }
    fn hook(&mut self, h: &Hook) -> Result<String, String> {
        self.hooks.push(h.clone());
        Ok("ok".into())
    }
    fn audit(&mut self, action: &str, detail: &str) {
        self.audits.push(format!("{action} {detail}"));
    }
}

fn deletes(f: &Fake, what: &str) -> bool {
    f.kube.iter().any(|k| k.contains("delete") && k.contains(what))
}

#[test]
fn every_item_has_a_plan_and_every_target_passes_the_checks() {
    let d = tmp("all");
    for it in Item::ALL {
        let p = plan(it, &ctx(&d));
        assert!(!p.steps.is_empty(), "{it:?} has no removal plan");
        for s in &p.steps {
            check_step(s, &d).unwrap_or_else(|e| panic!("{it:?}: {} -> {e}", s.text));
            assert!(!s.text.is_empty());
        }
        // no wildcard and no bulk delete anywhere
        for s in &p.steps {
            if let Op::Kube { name, .. } | Op::Claim { name, .. } = &s.op {
                assert!(!name.contains('*') && !name.contains(',') && !name.contains(' '), "{name}");
            }
        }
    }
}

#[test]
fn the_cluster_items_list_the_right_targets() {
    let d = tmp("targets");
    let kinds = |it: Item| -> Vec<String> { plan(it, &ctx(&d)).steps.iter().filter_map(|s| if let Op::Kube { kind, name, .. } = &s.op { Some(format!("{kind}/{name}")) } else { None }).collect() };
    assert_eq!(kinds(Item::Elasticsearch), vec!["deployment/elasticsearch", "service/elasticsearch"]);
    assert!(claims_of(Item::Elasticsearch) == vec![(NAMESPACE, "elasticsearch-data".to_string())]);
    assert!(kinds(Item::Prometheus).iter().any(|k| k == "clusterrole/iyakki-prometheus") && kinds(Item::Prometheus).iter().any(|k| k == "configmap/prometheus-config"));
    assert_eq!(claims_of(Item::Gitea), vec![(gitea::NAMESPACE, "gitea-data".to_string())]);
    assert_eq!(claims_of(Item::Lemonade), vec![(LEMONADE_NAMESPACE, "lemonade-data".to_string())]);
    for e in objstore::ENGINES {
        assert_eq!(claims_of(e.item), vec![(objstore::NS, e.claim())]);
        let p = plan(e.item, &ctx(&d));
        let vault: Vec<&str> = p.steps.iter().filter_map(|s| if let Op::Vault(v) = &s.op { Some(v.as_str()) } else { None }).collect();
        assert_eq!(vault, vec![e.access_vault(), e.secret_vault(), e.record_name()]);
    }
    // the same targets as the older object storage removal
    struct Old(Vec<String>);
    impl objstore::RemoveEnv for Old {
        fn kubectl(&mut self, a: &[&str]) -> Result<String, String> {
            self.0.push(a.join(" "));
            Ok(String::new())
        }
        fn unmark(&mut self, _: Item) {}
        fn vault_delete(&mut self, n: &str) -> Result<(), String> {
            self.0.push(n.into());
            Ok(())
        }
    }
    let mut old = Old(vec![]);
    objstore::remove(&objstore::SEAWEEDFS, &mut old, true).unwrap();
    for k in kinds(Item::SeaweedFs) {
        let (kind, name) = k.split_once('/').unwrap();
        assert!(old.0.iter().any(|l| l.contains(kind) && l.contains(name)), "{k}");
    }
}

#[test]
fn the_default_removal_keeps_the_data_the_vault_and_the_dash_connection() {
    let d = tmp("keep");
    for it in [Item::GraphDb, Item::ClickHouse, Item::Matrix, Item::Prometheus, Item::Gitea, Item::SeaweedFs, Item::Elasticsearch, Item::Lemonade, Item::Registry] {
        mark_done(&d, it, "x").unwrap();
        let mut f = Fake::default();
        let msg = run(&plan(it, &ctx(&d)), &mut f, &d, false).unwrap();
        assert!(deletes(&f, "deployment") || it == Item::Gitea, "{it:?}");
        assert!(!f.kube.iter().any(|k| k.contains("pvc") || k.contains("secret") || k.contains("namespace") || k.contains(" pv ")), "{it:?}: {:?}", f.kube);
        assert!(f.vault.is_empty() && f.sources.is_empty(), "{it:?}");
        assert!(!f.hooks.contains(&Hook::GiteaConnection));
        assert_eq!(marked(&d, it), None, "the item shows as not installed");
        assert!(msg.contains("kept"), "{msg}");
    }
}

#[test]
fn delete_data_removes_exactly_the_data() {
    let d = tmp("data");
    let mut f = Fake::default();
    run(&plan(Item::GraphDb, &ctx(&d)), &mut f, &d, true).unwrap();
    assert!(deletes(&f, "pvc graphdb-data") && deletes(&f, "pv pvc-1234") && deletes(&f, &format!("secret {GRAPH_SECRET}")));
    assert_eq!(f.vault, vec![GRAPH_VAULT_NAME.to_string()]);
    assert_eq!(f.sources, vec![GRAPH_SOURCE_ID.to_string()]);
    assert!(!deletes(&f, "namespace"), "iyakki-data is shared by other items");
    let mut m = Fake::default();
    run(&plan(Item::Matrix, &ctx(&d)), &mut m, &d, true).unwrap();
    assert!(deletes(&m, "namespace iyakki-matrix"), "its own namespace goes with its data");
    // nothing else is named
    assert!(m.kube.iter().all(|k| k.contains("matrix") || k.contains("pv pvc-1234")), "{:?}", m.kube);
    let mut g = Fake::default();
    run(&plan(Item::Gitea, &ctx(&d)), &mut g, &d, true).unwrap();
    assert!(g.vault.len() == 2 && g.hooks.contains(&Hook::GiteaConnection) && g.hooks.contains(&Hook::GiteaRecord));
}

#[test]
fn delete_data_deletes_the_claim_the_retained_volume_and_its_folder() {
    let d = tmp("pvfolder");
    let root = storage_root(&d);
    let keep = root.join("pvc-other_ns_claim");
    let mine = root.join("pvc-1234_iyakki-data_graphdb-data");
    let stray = root.join("pvc-1234x");
    for p in [&keep, &mine, &stray] {
        std::fs::create_dir_all(p.join("db")).unwrap();
    }
    struct Env(Fake, String);
    impl UninstallEnv for Env {
        fn kubectl(&mut self, a: &[&str]) -> Result<String, String> {
            let l = a.join(" ");
            if l.contains("spec.local.path") {
                self.0.kube.push(l);
                return Ok(self.1.clone());
            }
            self.0.kubectl(a)
        }
        fn vault_delete(&mut self, n: &str) -> Result<(), String> {
            self.0.vault_delete(n)
        }
        fn source_remove(&mut self, i: &str) -> Result<(), String> {
            self.0.source_remove(i)
        }
        fn hook(&mut self, h: &Hook) -> Result<String, String> {
            self.0.hook(h)
        }
        fn audit(&mut self, a: &str, b: &str) {
            self.0.audit(a, b)
        }
    }
    let mut e = Env(Fake::default(), mine.display().to_string());
    run(&plan(Item::GraphDb, &ctx(&d)), &mut e, &d, true).unwrap();
    assert!(!mine.exists(), "the folder of the retained volume is deleted");
    assert!(keep.exists() && stray.exists(), "other volumes are untouched");
    // a path the cluster reports outside the storage folder is never deleted
    let outside = tmp("pvoutside");
    std::fs::create_dir_all(outside.join("pvc-1234_a_b")).unwrap();
    let mut e = Env(Fake::default(), outside.join("pvc-1234_a_b").display().to_string());
    run(&plan(Item::GraphDb, &ctx(&d)), &mut e, &d, true).unwrap();
    assert!(outside.join("pvc-1234_a_b").exists());
}

#[test]
fn k3s_with_dependants_is_refused_with_their_names_or_removes_them_first() {
    let d = tmp("deps");
    let there = |i: Item| matches!(i, Item::K3s | Item::Lemonade | Item::Kokoro | Item::Registry | Item::Elasticsearch);
    let mut f = Fake::default();
    let err = uninstall(Item::K3s, Opts::default(), &there, &ctx(&d), &mut f).unwrap_err();
    for n in ["Lemonade", "Kokoro", "Local image registry", "Elasticsearch"] {
        assert!(err.contains(n), "{err}");
    }
    assert!(f.kube.is_empty() && f.hooks.is_empty() && f.audits.is_empty(), "nothing was changed");
    let order = cascade(Item::K3s, &there);
    assert!(order.iter().position(|i| *i == Item::Kokoro) < order.iter().position(|i| *i == Item::Lemonade), "Kokoro goes before Lemonade");
    // Kokoro is its own data: the cascade stops at it unless the data is chosen, and nothing after it ran
    let mut f = Fake::default();
    let e = uninstall(Item::Lemonade, Opts { delete_data: false, with_dependants: true }, &there, &ctx(&d), &mut f).unwrap_err();
    assert!(e.contains("Kokoro"), "{e}");
    let not_kokoro = |i: Item| matches!(i, Item::K3s | Item::Registry | Item::Elasticsearch);
    let mut f = Fake::default();
    uninstall(Item::K3s, Opts { delete_data: false, with_dependants: true }, &not_kokoro, &ctx(&d), &mut f).unwrap();
    assert!(f.hooks.contains(&Hook::K3sKeepData) && !f.hooks.contains(&Hook::K3sAll));
    let first_k3s = f.audits.iter().position(|a| a.contains("start k3s")).unwrap();
    assert!(f.audits.iter().position(|a| a.contains("start elasticsearch")).unwrap() < first_k3s && f.audits.iter().position(|a| a.contains("start registry")).unwrap() < first_k3s);
    assert!(!f.kube.iter().any(|k| k.contains("pvc")), "dependants keep their data");
    // with the data chosen, k3s is deleted whole
    let mut f = Fake::default();
    uninstall(Item::K3s, Opts { delete_data: true, with_dependants: false }, &|i| i == Item::K3s, &ctx(&d), &mut f).unwrap();
    assert!(f.hooks.contains(&Hook::K3sAll) && !f.hooks.contains(&Hook::K3sKeepData));
}

#[test]
fn an_item_other_tools_use_is_warned_about_and_a_foreign_cluster_is_never_removed() {
    // items that install through a runtime (when the catalogue has any) are warned about; the catalogue's own users come as cli_warning
    for rt in [Item::Node, Item::Python] {
        for user in Item::ALL.into_iter().filter(|i| *i != rt && i.needs_now().contains(&rt)) {
            let there = move |i: Item| i == rt || i == user;
            assert_eq!(soft_users(rt, &there), vec![user]);
            assert!(direct_dependants(rt, &there).is_empty());
            assert!(dependency_lines(rt, &there).1[0].contains(user.title()));
        }
    }
    let mut c = ctx(&tmp("warn"));
    c.cli_warning = Some("Node.js is used by Pi; they stop working if it is removed.".into());
    assert!(plan(Item::Node, &c).review(false).lines(false).join("\n").contains("is used by Pi"));
    assert!(not_ours(&Presence::Elsewhere("/usr/local/bin/k3s".into())).unwrap().contains("will not remove"));
    assert!(not_ours(&Presence::Installed("x".into())).is_none());
}

#[test]
fn a_target_outside_the_items_own_places_is_refused_and_nothing_runs() {
    let d = tmp("outside");
    let other = tmp("elsewhere");
    let bad = |op: Op| Step { op, data: false, unless_data: false, text: "x".into() };
    for op in [
        Op::Kube { ns: Some("kube-system"), kind: "deployment", name: "coredns".into() },
        Op::Kube { ns: Some("default"), kind: "deployment", name: "x".into() },
        Op::Kube { ns: Some("iyakki-data"), kind: "deployment", name: "*".into() },
        Op::Kube { ns: Some("iyakki-data"), kind: "deployment", name: "a,b".into() },
        Op::Kube { ns: Some("iyakki-data"), kind: "node", name: "n".into() },
        Op::Kube { ns: Some("iyakki-data"), kind: "namespace", name: "iyakki-data".into() },
        Op::Kube { ns: None, kind: "clusterrole", name: "cluster-admin".into() },
        Op::Claim { ns: "kube-system", name: "x".into() },
        Op::Vault("password".into()),
        Op::Vault("svc.*".into()),
        Op::Path { path: other.clone(), root: other.clone() },
        Op::Path { path: d.clone(), root: d.clone() },
        Op::Path { path: d.join("a/../../x"), root: d.join("a") },
        Op::Path { path: d.join("b"), root: d.join("a") },
        Op::Hook(Hook::Sandbox("mine")),
        Op::Hook(Hook::DriverPack("fw-other")),
    ] {
        assert!(check_step(&bad(op.clone()), &d).is_err(), "{op:?} must be refused");
    }
    std::fs::write(other.join("keep.txt"), "x").unwrap();
    let evil = Plan { item: Item::Ffmpeg, steps: vec![bad(Op::Hook(Hook::Cli("ffmpeg"))), bad(Op::Path { path: other.clone(), root: other.clone() })], data_requires: false, always_stays: vec![], claim_gb: 0, data_bytes: 0, notes: vec![] };
    let mut f = Fake::default();
    assert!(run(&evil, &mut f, &d, false).is_err());
    assert!(f.hooks.is_empty() && other.join("keep.txt").exists(), "the first step did not run either: all targets are checked before the first change");
}

#[test]
fn running_twice_is_harmless_and_files_go_only_under_their_own_folder() {
    let d = tmp("twice");
    let addon = crate::avaddon::addon_dir(&d);
    std::fs::create_dir_all(addon.join("lib")).unwrap();
    std::fs::write(addon.join("NOTICE"), "n").unwrap();
    std::fs::write(d.join("precious.txt"), "mine").unwrap();
    mark_done(&d, Item::FfmpegLibs, "x").unwrap();
    let p = plan(Item::FfmpegLibs, &ctx(&d));
    let mut f = Fake::default();
    run(&p, &mut f, &d, false).unwrap();
    assert!(!addon.exists() && d.join("precious.txt").exists());
    run(&p, &mut f, &d, false).unwrap();
    // the cluster items twice: only `--ignore-not-found` deletes
    let g = plan(Item::GraphDb, &ctx(&d));
    run(&g, &mut f, &d, true).unwrap();
    run(&g, &mut f, &d, true).unwrap();
    assert!(f.kube.iter().filter(|k| k.contains("delete")).all(|k| k.contains("--ignore-not-found")));
}

#[test]
fn the_nvidia_download_stays_unless_the_data_is_chosen() {
    let d = tmp("nv");
    let l = crate::gpu_nvidia::Layout::new(&d);
    for p in [l.stage(), l.extracted()] {
        std::fs::create_dir_all(&p).unwrap();
    }
    std::fs::write(l.run(), "run").unwrap();
    let mut f = Fake::default();
    run(&plan(Item::GpuNvidia, &ctx(&d)), &mut f, &d, false).unwrap();
    assert!(!l.stage().exists() && !l.extracted().exists() && l.run().exists());
    assert!(f.kube.iter().any(|k| k.contains("kube-system") && k.contains("nvidia-device-plugin")));
    run(&plan(Item::GpuNvidia, &ctx(&d)), &mut f, &d, true).unwrap();
    assert!(!l.run().exists());
}

#[test]
fn a_half_installed_item_can_be_removed_and_a_failure_keeps_the_marker_for_a_retry() {
    let d = tmp("half");
    // no objects exist in the cluster, only a marker
    mark_done(&d, Item::ClickHouse, "x").unwrap();
    let mut f = Fake::default();
    run(&plan(Item::ClickHouse, &ctx(&d)), &mut f, &d, false).unwrap();
    assert_eq!(marked(&d, Item::ClickHouse), None);
    // a failure in the middle
    mark_done(&d, Item::ClickHouse, "x").unwrap();
    let mut f = Fake { fail_on: Some("service".into()), ..Default::default() };
    let e = run(&plan(Item::ClickHouse, &ctx(&d)), &mut f, &d, false).unwrap_err();
    assert!(e.contains("Retry") && e.contains("Service clickhouse"), "{e}");
    assert!(marked(&d, Item::ClickHouse).is_some(), "still shown, so Retry is offered");
    assert!(f.audits.iter().any(|a| a.starts_with("quickinstall.uninstall.failed")));
    let mut f = Fake::default();
    run(&plan(Item::ClickHouse, &ctx(&d)), &mut f, &d, false).unwrap();
    assert_eq!(marked(&d, Item::ClickHouse), None);
}

#[test]
fn an_item_that_is_its_own_data_needs_the_data_choice() {
    let d = tmp("own");
    for it in [Item::Kokoro, Item::VoicePack, Item::LinuxApps, Item::LinuxWindows] {
        let p = plan(it, &ctx(&d));
        assert!(p.data_requires);
        let mut f = Fake::default();
        assert!(run(&p, &mut f, &d, false).unwrap_err().contains("delete the data"));
        assert!(f.hooks.is_empty());
        assert!(p.review(false).space.is_some(), "the card says how much the sandbox or model frees");
        run(&p, &mut f, &d, true).unwrap();
        assert_eq!(f.hooks.len(), 1);
    }
}

#[test]
fn the_audit_lines_name_the_action_and_the_item_and_hold_no_secret_or_path() {
    let d = tmp("audit");
    let mut f = Fake::default();
    run(&plan(Item::SeaweedFs, &ctx(&d)), &mut f, &d, true).unwrap();
    assert_eq!(f.audits.len(), 2);
    for a in &f.audits {
        assert!(a.starts_with("quickinstall.uninstall.") && a.contains("seaweedfs"), "{a}");
        assert!(!a.contains("svc.") && !a.contains("s3.") && !a.contains('/'), "{a}");
    }
    assert!(f.audits[0].contains("data=deleted"));
}

#[test]
fn the_review_says_what_goes_what_stays_the_space_and_that_it_cannot_be_undone() {
    let d = tmp("review");
    let p = plan(Item::Elasticsearch, &ctx(&d));
    let r = p.review(false);
    let t = r.lines(false).join("\n");
    assert!(t.contains("Deployment elasticsearch") && t.contains("elasticsearch-data") && t.contains("(kept)") && t.contains("cannot be undone") && t.contains("5 GB"), "{t}");
    assert!(r.data.is_empty());
    let t = p.review(true).lines(true).join("\n");
    assert!(t.contains("ALSO DELETED") && t.contains("cannot be undone") && t.contains("DASH connection"), "{t}");
    // where the data lives, and the volatile warning
    assert!(t.contains("Retain") || r.notes.join(" ").contains("Retain"));
    assert!(!r.notes.join(" ").contains("WARNING"));
    let mut c = ctx(&d);
    c.volatile = true;
    let v = plan(Item::Elasticsearch, &c).review(false);
    assert!(v.notes.iter().any(|n| n.contains("WARNING") && n.contains("no persistent data volume")));
}

#[test]
fn the_volumes_are_set_to_retain_once_and_a_second_pass_changes_nothing() {
    let mut patched: Vec<String> = vec![];
    let mut policy = "Delete".to_string();
    let claims = vec![(NAMESPACE, "graphdb-data".to_string()), (NAMESPACE, "unbound-data".to_string())];
    let mut k = |a: &[&str]| -> Result<String, String> {
        let l = a.join(" ");
        if l.contains("graphdb-data") {
            Ok("pvc-aa".into())
        } else if l.contains("unbound-data") {
            Ok(String::new())
        } else if l.starts_with("get pv") {
            Ok(policy.clone())
        } else if l.starts_with("patch pv") {
            assert!(l.contains("Retain"));
            patched.push(l);
            policy = "Retain".into();
            Ok(String::new())
        } else {
            Err("?".into())
        }
    };
    assert_eq!(retain_claims(&mut k, &claims), vec!["pvc-aa".to_string()]);
    assert!(retain_claims(&mut k, &claims).is_empty(), "idempotent");
    assert_eq!(patched.len(), 1);
    // a cluster that does not answer is skipped, not an error
    let mut dead = |_: &[&str]| -> Result<String, String> { Err("down".into()) };
    assert!(retain_claims(&mut dead, &claims).is_empty());
}

#[test]
fn a_reinstall_reuses_the_same_claim_with_the_same_size() {
    let d = tmp("reinstall");
    let cases: Vec<(Item, String)> = vec![
        (Item::Elasticsearch, elasticsearch_manifest()),
        (Item::GraphDb, graphdb_manifest()),
        (Item::Prometheus, prometheus_manifest()),
        (Item::ClickHouse, clickhouse::manifest()),
        (Item::OpenSearch, opensearch::manifest()),
        (Item::Matrix, matrix::manifest()),
        (Item::SeaweedFs, objstore::manifest(&objstore::SEAWEEDFS)),
        (Item::RustFs, objstore::manifest(&objstore::RUSTFS)),
        (Item::Lemonade, lemonade_manifest()),
    ];
    for (it, manifest) in cases {
        mark_done(&d, it, "x").unwrap();
        let mut f = Fake::default();
        run(&plan(it, &ctx(&d)), &mut f, &d, false).unwrap();
        assert!(!f.kube.iter().any(|k| k.contains("pvc")), "{it:?}: the removal never touches the claim");
        for (ns, claim) in claims_of(it) {
            assert!(manifest.contains(&format!("name: {claim}")) && manifest.contains(&format!("namespace: {ns}")) || manifest.contains(&format!("name: {claim}")), "{it:?}: the installer applies {claim}");
            // one claim of one size, so applying again binds the same claim
            assert_eq!(manifest.matches("kind: PersistentVolumeClaim").count(), 1, "{it:?}");
            let gb: u32 = manifest.split("storage: ").nth(1).and_then(|r| r.split("Gi").next()).and_then(|n| n.trim().parse().ok()).unwrap();
            assert_eq!(gb, plan(it, &ctx(&d)).claim_gb, "{it:?}: the card's size is the manifest's size");
        }
    }
}

// ------------------------------------------------------------------------------------------------ Ollama in the cluster

#[test]
fn ollama_in_the_cluster_keeps_the_models_claim_by_default_and_deletes_it_with_the_data() {
    let d = tmp("ollama-cluster");
    mark_done(&d, Item::Ollama, &ollama_cluster::marker_detail(1, "")).unwrap();
    let pl = plan(Item::Ollama, &ctx(&d));
    for s in &pl.steps {
        check_step(s, &d).unwrap();
    }
    let kube_names: Vec<String> = pl.steps.iter().filter_map(|s| match &s.op { Op::Kube { kind, name, .. } => Some(format!("{kind}/{name}")), Op::Claim { name, .. } => Some(format!("claim/{name}")), _ => None }).collect();
    assert!(kube_names.contains(&"deployment/ollama".to_string()) && kube_names.contains(&"service/ollama".to_string()) && kube_names.contains(&"claim/ollama-data".to_string()));
    assert!(pl.steps.iter().any(|s| s.op == Op::Hook(Hook::OllamaProvider) && !s.data), "ollama-local leaves Models");
    assert!(!pl.steps.iter().any(|s| s.op == Op::Hook(Hook::Cli("ollama"))), "no host program is touched for the cluster shape");
    let r = pl.review(false);
    assert!(r.goes.iter().any(|g| g.contains("Deployment ollama")) && r.stays.iter().any(|s| s.contains("ollama-data") && s.contains("kept")), "{r:?}");
    let r = pl.review(true);
    assert!(r.data.iter().any(|g| g.contains("ollama-data")), "{r:?}");
    assert_eq!(pl.claim_gb, 30);
    assert!(pl.always_stays.iter().any(|s| s.contains("iyakki-ai")), "the shared namespace stays");
}

#[test]
fn a_moved_ollama_offers_the_earlier_host_models_folder_only_as_data_and_never_by_default() {
    let d = tmp("ollama-moved");
    std::fs::create_dir_all(d.join("ollama/models/blobs")).unwrap();
    std::fs::write(d.join("ollama/models/blobs/x"), vec![0u8; 4096]).unwrap();
    mark_done(&d, Item::Ollama, &ollama_cluster::marker_detail(1, "; the host models were copied in (the host folder is kept)")).unwrap();
    let pl = plan(Item::Ollama, &ctx(&d));
    let host = pl.steps.iter().find(|s| matches!(&s.op, Op::Path { .. })).expect("the host folder step");
    assert!(host.data, "the earlier folder goes only with the data");
    check_step(host, &d).unwrap();
    assert!(pl.review(false).stays.iter().any(|s| s.contains("earlier host Ollama")));
    assert!(pl.notes.iter().any(|n| n.contains("stays installed")));
}

#[test]
fn an_ollama_on_the_host_still_has_its_old_removal_plan() {
    let d = tmp("ollama-host");
    mark_done(&d, Item::Ollama, "version 0.5.1").unwrap();
    let pl = plan(Item::Ollama, &ctx(&d));
    assert!(pl.steps.iter().any(|s| s.op == Op::Hook(Hook::Cli("ollama"))) && pl.steps.iter().any(|s| s.op == Op::Hook(Hook::OllamaServer)));
    assert!(pl.steps.iter().any(|s| s.data && matches!(&s.op, Op::Path { .. })));
    assert!(!pl.steps.iter().any(|s| matches!(&s.op, Op::Claim { .. })));
}

#[test]
fn the_gpu_removal_also_takes_the_gpu_out_of_the_ollama_deployment() {
    let d = tmp("ollama-gpu");
    for item in [Item::GpuVulkan, Item::GpuNvidia] {
        let pl = plan(item, &ctx(&d));
        assert!(pl.steps.iter().any(|s| s.op == Op::Hook(Hook::OllamaUnpatch)), "{item:?}");
        for s in &pl.steps {
            check_step(s, &d).unwrap();
        }
    }
}

#[test]
fn the_ollama_claim_is_retained_whichever_shape_the_marker_says() {
    assert_eq!(claims_of(Item::Ollama), vec![("iyakki-ai", "ollama-data".to_string())]);
    let mut calls: Vec<String> = vec![];
    let mut k = |a: &[&str]| -> Result<String, String> {
        calls.push(a.join(" "));
        Ok(if a.join(" ").contains("spec.volumeName") { "pvc-9".into() } else { "Delete".into() })
    };
    let done = retain_claims(&mut k, &claims_of(Item::Ollama));
    assert_eq!(done, ["pvc-9"]);
    assert!(calls.iter().any(|c| c.contains("-n iyakki-ai get pvc ollama-data")) && calls.iter().any(|c| c.contains("patch pv pvc-9") && c.contains("Retain")));
}
