// #[cfg(test)] -- this whole file is test code (kubeprofiles.rs includes it under #[cfg(test)]); the marker keeps the BOUNDARY check from counting it.
//! Cloud mode: the in-cluster service account profile (fake environment and files), what the account may do, and the lifecycle of
//! a kubeconfig held in a vault (a 0600 temp file for one child, gone afterwards).

use super::*;
use crate::devcluster::{self, Env, Source};
use crate::wsexec::{FakeRunner, System};
use std::os::unix::fs::PermissionsExt;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-kubeprofiles-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const TOKEN: &str = "eyJhbGciOiJSUzI1NiJ9.SUPER-SECRET-SERVICE-ACCOUNT-TOKEN.sig";

fn pod(d: &Path, host: Option<&str>) -> PodEnv {
    let sa = d.join("sa");
    std::fs::create_dir_all(&sa).unwrap();
    std::fs::write(sa.join("token"), TOKEN).unwrap();
    std::fs::write(sa.join("ca.crt"), "-----BEGIN CERTIFICATE-----\nx\n-----END CERTIFICATE-----\n").unwrap();
    std::fs::write(sa.join("namespace"), "iyakki\n").unwrap();
    PodEnv { host: host.map(str::to_string), port: Some("443".into()), token: sa.join("token"), ca: sa.join("ca.crt"), namespace: sa.join("namespace"), out: d.join("incluster.yaml") }
}

const CAN_I_ROLE: &str = "Resources                                       Non-Resource URLs   Resource Names   Verbs\nselfsubjectreviews.authentication.k8s.io        []                  []               [create]\nselfsubjectaccessreviews.authorization.k8s.io   []                  []               [create]\n                                                [/healthz]          []               [get]\npods                                            []                  []               [get list watch]\ndeployments.apps                                []                  []               [get list watch create update patch]\nconfigmaps                                      []                  []               [get list watch create]\n";
const CAN_I_ADMIN: &str = "Resources   Non-Resource URLs   Resource Names   Verbs\n*.*         []                  []               [*]\n            [*]                 []               [*]\n";

#[test]
fn a_pod_with_a_service_account_is_detected_from_the_environment_and_the_mounted_files_only() {
    let d = tmp("detect");
    let ic = detect_in_cluster(&pod(&d, Some("10.43.0.1"))).expect("in a pod");
    assert_eq!((ic.server.as_str(), ic.namespace.as_str()), ("https://10.43.0.1:443", "iyakki"));
    // IPv6 service addresses are bracketed
    assert_eq!(detect_in_cluster(&pod(&d, Some("fd00::1"))).unwrap().server, "https://[fd00::1]:443");
    // not in a pod: no variable, an empty one, no token, an empty token, no CA
    assert!(detect_in_cluster(&pod(&d, None)).is_none());
    assert!(detect_in_cluster(&pod(&d, Some(""))).is_none());
    let mut e = pod(&d, Some("10.43.0.1"));
    e.token = d.join("sa/nope");
    assert!(detect_in_cluster(&e).is_none(), "no token mounted (automountServiceAccountToken: false)");
    let e = pod(&d, Some("10.43.0.1"));
    std::fs::write(&e.token, "\n").unwrap();
    assert!(detect_in_cluster(&e).is_none(), "an empty token is no token");
    let mut e = pod(&d, Some("10.43.0.1"));
    e.ca = d.join("sa/nope.crt");
    assert!(detect_in_cluster(&e).is_none(), "without the CA the server cannot be verified, so no profile is offered");
    // a missing namespace file means `default`
    let mut e = pod(&d, Some("10.43.0.1"));
    e.namespace = d.join("sa/none");
    assert_eq!(detect_in_cluster(&e).unwrap().namespace, "default");
}

#[test]
fn the_generated_kubeconfig_names_the_token_file_never_the_token_and_is_private() {
    let d = tmp("config");
    let e = pod(&d, Some("10.43.0.1"));
    let path = in_cluster_kubeconfig_for(&e).unwrap();
    let t = std::fs::read_to_string(&path).unwrap();
    assert!(t.contains("server: https://10.43.0.1:443") && t.contains(&format!("tokenFile: {}", e.token.display())) && t.contains(&format!("certificate-authority: {}", e.ca.display())) && t.contains("namespace: iyakki"), "{t}");
    assert!(!t.contains("SUPER-SECRET") && !t.contains(TOKEN), "the token is read by kubectl from its file, never copied");
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(in_cluster_kubeconfig_for(&pod(&d, None)).is_none(), "not in a pod: nothing is written");
}

#[test]
fn what_the_service_account_may_do_is_listed_and_an_account_that_can_do_everything_is_flagged() {
    let l = summarize_can_i(CAN_I_ROLE);
    assert_eq!(l, vec!["selfsubjectreviews.authentication.k8s.io: create", "selfsubjectaccessreviews.authorization.k8s.io: create", "pods: get list watch", "deployments.apps: get list watch create update patch", "configmaps: get list watch create"]);
    assert!(!cluster_admin_like(&l));
    let t = can_i_text(&l);
    assert!(t.contains("pods: get list watch") && t.contains("deployments.apps: get list watch create update patch") && !t.contains("selfsubject") && !t.contains("cluster-admin"), "{t}");
    let a = summarize_can_i(CAN_I_ADMIN);
    assert_eq!(a, vec!["*.*: *"]);
    assert!(cluster_admin_like(&a));
    assert!(can_i_text(&a).contains("cluster-admin like: the assistant only reads"));
    assert_eq!(can_i_text(&[]), "no permissions listed");
}

fn world_answers(run: &FakeRunner, can_i: &str) {
    run.answer("config current-context", "in-cluster\n");
    run.answer("config view --minify", "https://10.43.0.1:443");
    run.answer("version -o json", "{\"serverVersion\":{\"gitVersion\":\"v1.31.1+k3s1\"}}");
    run.answer("auth can-i --list", can_i);
}

#[test]
fn the_resolver_offers_an_in_cluster_profile_with_the_roles_permissions_and_the_assistant_writes_only_as_far_as_they_go() {
    let d = tmp("resolve");
    let e = pod(&d, Some("10.43.0.1"));
    let kc = in_cluster_kubeconfig_for(&e).unwrap();
    let env = Env { home: d.join("nohome"), extra: vec![(Source::InCluster, kc.clone())], ..Default::default() };
    for (can_i, admin) in [(CAN_I_ROLE, false), (CAN_I_ADMIN, true)] {
        let fake = Arc::new(FakeRunner::new(&["kubectl"]));
        world_answers(&fake, can_i);
        let run: Arc<dyn Runner> = fake.clone();
        let res = devcluster::resolve(&run, &env, &crate::workspace::K8sConfig::default(), None, "shop-dev", false);
        let p = res.chosen().expect("the in-cluster profile is the one that answers");
        assert_eq!((p.source, p.reachable, p.kubeconfig.as_str()), (Source::InCluster, true, kc.display().to_string().as_str()));
        assert_eq!(Source::InCluster.label(), "this pod's service account");
        // the permissions come from `kubectl auth can-i --list` in the workspace namespace
        assert!(fake.lines().iter().any(|l| l.contains("auth can-i --list -n shop-dev")), "{:?}", fake.lines());
        assert!(admin || p.can_i.contains("deployments.apps: get list watch create update patch"), "{}", p.can_i);
        assert_eq!(p.can_i.contains("cluster-admin like"), admin);
        // the tools: a Role-limited account may be asked to write (with the usual approval); a cluster-admin one is read-only
        let t = crate::k8stools::K8sTools::new(run.clone(), p.clone(), "shop-dev", "shop", &d, Arc::new(|_, _| {}));
        let line = t.describe("k8s_apply", &serde_json::json!({"yaml": "kind: ConfigMap"}));
        assert_eq!(line.contains("REFUSED"), admin, "{line}");
    }
}

#[test]
fn a_kubeconfig_held_in_a_vault_is_a_private_temp_file_for_one_child_and_gone_afterwards() {
    let d = tmp("vault");
    let secret = "apiVersion: v1\nkind: Config\n# SECRET-KUBECONFIG-BODY\n";
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    let resolver: Resolver = Arc::new(move |r| {
        s2.lock().unwrap().push(r.to_string());
        if r == "uvault:prod-eks" { Ok(secret.to_string()) } else { Err("no such entry".into()) }
    });
    // no reference: nothing to do
    assert!(prepare_in(&["kubectl".into(), "get".into(), "pods".into()], &[], &d, Some(&resolver)).unwrap().is_none());
    assert!(seen.lock().unwrap().is_empty(), "the vault is not touched when no profile uses it");
    // `--kubeconfig <ref>`
    let argv: Vec<String> = ["kubectl", "--kubeconfig", "uvault:prod-eks", "--context", "eks", "get", "pods"].iter().map(|s| s.to_string()).collect();
    let (a, e, guards) = prepare_in(&argv, &[("KUBECONFIG".into(), "uvault:prod-eks".into()), ("HOME".into(), "/x".into())], &d, Some(&resolver)).unwrap().unwrap();
    assert_eq!(guards.len(), 2, "one temp file per use");
    let path = PathBuf::from(&a[2]);
    assert!(path.starts_with(&d) && path.file_name().unwrap().to_string_lossy().starts_with("iyakki-kc-"), "{a:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), secret);
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "private while it exists");
    assert_eq!((a[0].as_str(), a[1].as_str(), a[3].as_str()), ("kubectl", "--kubeconfig", "--context"));
    assert!(e.iter().any(|(k, v)| k == "KUBECONFIG" && PathBuf::from(v).exists()) && e.iter().any(|(k, v)| k == "HOME" && v == "/x"));
    assert!(!a.iter().any(|x| x.contains("SECRET")) && !a.join(" ").contains("uvault"), "the reference is gone from the command line: {a:?}");
    drop(guards);
    assert!(!path.exists(), "removed as soon as the child is done");
    assert_eq!(std::fs::read_dir(&d).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("iyakki-kc-")).count(), 0);
    // `--kubeconfig=<ref>`
    let (a, _, g) = prepare_in(&["kubectl".into(), "--kubeconfig=uvault:prod-eks".into()], &[], &d, Some(&resolver)).unwrap().unwrap();
    assert!(a[1].starts_with("--kubeconfig=") && PathBuf::from(&a[1]["--kubeconfig=".len()..]).exists());
    drop(g);
    // a locked vault or an unknown entry: an error that names the entry, never the secret; no file is left
    let e = prepare_in(&["kubectl".into(), "--kubeconfig".into(), "uvault:missing".into()], &[], &d, Some(&resolver)).unwrap_err();
    assert!(e.contains("uvault:missing") && e.contains("could not be read from the vault") && !e.contains("SECRET"), "{e}");
    let e = prepare_in(&["kubectl".into(), "--kubeconfig".into(), "vault:x".into()], &[], &d, None).unwrap_err();
    assert!(e.contains("nobody is signed in"), "{e}");
    let empty: Resolver = Arc::new(|_| Ok("  \n".into()));
    assert!(prepare_in(&["kubectl".into(), "--kubeconfig".into(), "vault:x".into()], &[], &d, Some(&empty)).unwrap_err().contains("is empty"));
    assert_eq!(std::fs::read_dir(&d).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("iyakki-kc-")).count(), 0, "failures leave no file");
}

#[test]
fn the_real_runner_swaps_the_reference_for_a_temp_file_runs_the_child_and_removes_the_file() {
    let secret = "VAULT-HELD-KUBECONFIG-TEXT";
    let resolver: Resolver = Arc::new(move |r| if r == "uvault:dev" { Ok(secret.into()) } else { Err("locked".into()) });
    set_resolver(Some(resolver));
    let o = System.run(&std::env::temp_dir(), &["sh".into(), "-c".into(), "cat \"$KUBECONFIG\"; echo; echo \"$KUBECONFIG\"".into()], &[("KUBECONFIG".into(), "uvault:dev".into())], Duration::from_secs(5), 4096, None);
    assert!(o.ok(), "{o:?}");
    let mut lines = o.stdout.lines();
    assert_eq!(lines.next(), Some(secret), "the child could read the kubeconfig");
    let path = lines.next().unwrap();
    assert!(path.contains("iyakki-kc-") && !Path::new(path).exists(), "{path}: removed after the child ended");
    // with the vault locked the child is not started at all
    set_resolver(None);
    let o = System.run(&std::env::temp_dir(), &["sh".into(), "-c".into(), "echo should-not-run".into()], &[("KUBECONFIG".into(), "uvault:dev".into())], Duration::from_secs(5), 4096, None);
    assert!(!o.ok() && !o.stdout.contains("should-not-run") && o.stderr.contains("nobody is signed in"), "{o:?}");
    // other programs are untouched
    assert_eq!(System.run(&std::env::temp_dir(), &["echo".into(), "hi".into()], &[], Duration::from_secs(5), 4096, None).stdout, "hi\n");
}

#[test]
fn stale_temp_files_of_a_process_that_died_are_swept_and_the_resolver_reads_the_right_vault() {
    let d = tmp("sweep");
    std::fs::write(d.join("iyakki-kc-old.yaml"), "x").unwrap();
    std::fs::write(d.join("other.yaml"), "x").unwrap();
    sweep_stale(&d, Duration::from_secs(3600));
    assert!(d.join("iyakki-kc-old.yaml").exists(), "a fresh file is not touched");
    std::thread::sleep(Duration::from_millis(30));
    sweep_stale(&d, Duration::from_millis(10));
    assert!(!d.join("iyakki-kc-old.yaml").exists() && d.join("other.yaml").exists(), "only ours, only old ones");

    struct Offline;
    impl iyakki_proxy::Upstream for Offline {
        fn request(&self, _m: &str, _u: &str, _h: &[(String, String)], _b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }
    let svc = crate::services::Services::open(&d.join("svc"), Arc::new(Offline), Arc::new(Offline)).unwrap();
    svc.on_login("amy", "amy-pass-123", true);
    svc.user_set("amy", "dev-kc", "AMYS-KUBECONFIG").unwrap();
    svc.system_set("shared-kc", "SHARED-KUBECONFIG").unwrap();
    let r = resolver_for(&svc, "amy");
    assert_eq!(r("uvault:dev-kc").unwrap(), "AMYS-KUBECONFIG");
    assert_eq!(r("vault:shared-kc").unwrap(), "SHARED-KUBECONFIG");
    assert!(r("uvault:nope").is_err());
    let bob = resolver_for(&svc, "bob");
    assert!(bob("uvault:dev-kc").is_err(), "someone else's vault is not readable");
}

#[test]
fn vault_profiles_are_kept_per_person_with_checked_names_and_never_hold_the_secret() {
    let d = tmp("profiles");
    assert!(vault_load(&d, "amy").is_empty());
    vault_add(&d, "amy", "Staging EKS", "uvault:staging-eks").unwrap();
    vault_add(&d, "amy", "Shared prod", "vault:prod-kc").unwrap();
    vault_add(&d, "amy", "Bare name", "bare-entry").unwrap();
    let l = vault_load(&d, "amy");
    assert_eq!(l.iter().map(|p| (p.name.as_str(), p.reference.as_str())).collect::<Vec<_>>(), vec![("Staging EKS", "uvault:staging-eks"), ("Shared prod", "vault:prod-kc"), ("Bare name", "uvault:bare-entry")]);
    assert!(vault_load(&d, "bob").is_empty(), "per person");
    vault_add(&d, "amy", "Staging EKS", "uvault:other").unwrap();
    assert_eq!(vault_load(&d, "amy").len(), 3, "the same name replaces");
    for bad in [("", "uvault:x"), ("a/b", "uvault:x"), ("ok", "uvault:Bad Name"), ("ok", "uvault:")] {
        assert!(vault_add(&d, "amy", bad.0, bad.1).is_err(), "{bad:?}");
    }
    let raw = std::fs::read_to_string(d.join("devcluster-vault-amy.json")).unwrap();
    assert!(raw.contains("uvault:other") && !raw.contains("kind: Config"), "only references are stored");
    vault_remove(&d, "amy", "Shared prod").unwrap();
    assert_eq!(vault_load(&d, "amy").len(), 2);
    assert!(is_vault_ref("uvault:x") && is_vault_ref("vault:x") && !is_vault_ref("/etc/rancher/k3s/k3s.yaml"));
}

#[test]
fn a_vault_held_profile_is_inspected_without_a_file_and_answers_through_the_runner() {
    let d = tmp("inspect");
    let fake = Arc::new(FakeRunner::new(&["kubectl"]));
    world_answers(&fake, CAN_I_ROLE);
    let run: Arc<dyn Runner> = fake.clone();
    let env = Env { home: d.join("nohome"), extra: vec![(Source::Vault, PathBuf::from("uvault:staging"))], ..Default::default() };
    let res = devcluster::resolve(&run, &env, &crate::workspace::K8sConfig::default(), None, "ns", false);
    let p = res.chosen().expect("listed although it is not a file");
    assert_eq!((p.source, p.readable, p.reachable, p.kubeconfig.as_str()), (Source::Vault, true, true, "uvault:staging"));
    assert!(fake.calls().iter().all(|c| c.argv.contains(&"uvault:staging".to_string()) || c.env.iter().any(|(_, v)| v == "uvault:staging")), "kubectl is always given the reference; the runner swaps it");
}
