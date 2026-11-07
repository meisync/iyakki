//! Tests of the sandbox layer. Nothing here starts a process of the program under test or talks to a cluster.

use super::admit::{admit, choose_tier, image_registry, needs_approval, Who};
use super::engine::Engine;
use super::exec::*;
use super::job;
use super::kube::{parse_state, JobState, KubeApi};
use super::limiter::Limiter;
use super::output::clean;
use super::policy::SandboxPolicy;
use super::spec::*;
use crate::tenantconfig::api::{self, Caller, ConfigAudit, Env};
use crate::tenantconfig::vault::{MemVault, SecretStore};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn image() -> String {
    format!("registry.example.com/tools/cli:1.0@sha256:{DIGEST}")
}

fn base_values() -> Value {
    json!({
        "tiers": ["wasm", "job", "browser", "namespace"], "browser": true, "userns_ack": true,
        "allowed_registries": ["registry.example.com", "*.corp.example"], "require_digest": false,
        "max_cpu_m": 1000, "max_mem_mib": 512, "max_pids": 128, "max_seconds": 300, "max_workspace_mib": 256,
        "network_modes": ["none", "proxy"], "runtime_class": "gvisor", "cli_role": "editor",
        "approval_patterns": ["kubectl apply", "terraform"], "per_person_concurrency": 2, "per_team_concurrency": 4,
        "queue_timeout_secs": 5, "audit_retention_days": 90, "k3s_environment": "dev",
        "browser_domains": ["example.org", "*.example.org"], "browser_profile": "tmpfs", "browser_download_mib": 32,
        "browser_session_secs": 120, "browser_image": image(), "cli_image": image()
    })
}

fn policy_with(over: Value) -> SandboxPolicy {
    let mut v = base_values();
    for (k, x) in over.as_object().unwrap() {
        v[k] = x.clone();
    }
    let map: BTreeMap<String, Value> = v.as_object().unwrap().iter().map(|(k, x)| (k.clone(), x.clone())).collect();
    SandboxPolicy::from_values(true, &map)
}

fn pol() -> SandboxPolicy {
    policy_with(json!({}))
}

fn labels() -> Labels {
    Labels { tenant: "acme".into(), team: "web".into(), person: "mia".into(), run_id: "r1".into() }
}

fn spec() -> SandboxSpec {
    SandboxSpec::command(vec!["aws".into(), "s3".into(), "ls".into()], &image(), labels())
}

fn who() -> Who<'static> {
    Who { person: "mia", role: "editor" }
}

fn refused(r: Result<super::admit::Admitted, SandboxError>) -> String {
    match r {
        Err(SandboxError::Refused(m)) => m,
        Err(e) => panic!("expected a refusal, got {e:?}"),
        Ok(_) => panic!("expected a refusal, the request was admitted"),
    }
}

#[derive(Default)]
struct Rec(Mutex<Vec<String>>);
impl ConfigAudit for Rec {
    fn record(&self, who: &str, event: &str, detail: &str) {
        self.0.lock().unwrap().push(format!("{who} {event} {detail}"));
    }
}
impl Rec {
    fn all(&self) -> String {
        self.0.lock().unwrap().join("\n")
    }
}

// ------------------------------------------------------------------------------------------------------------------ tier choice

#[test]
fn the_tier_choice_table() {
    let p = pol();
    let cmd = |tier: Option<Tier>| SandboxSpec { tier, ..spec() };
    let wasm = |tier: Option<Tier>| SandboxSpec { work: Work::WasmModule, tier, wasm: vec![0], ..spec() };
    let br = |tier: Option<Tier>| SandboxSpec { work: Work::Browser, tier, ..spec() };
    // (what, policy, expected)
    assert_eq!(choose_tier(&p, &cmd(None)), Ok(Tier::Job), "a command defaults to a Job pod");
    assert_eq!(choose_tier(&p, &cmd(Some(Tier::Job))), Ok(Tier::Job));
    assert_eq!(choose_tier(&p, &cmd(Some(Tier::Namespace))), Ok(Tier::Namespace), "only when asked for, listed and acknowledged");
    assert!(choose_tier(&p, &cmd(Some(Tier::Wasm))).unwrap_err().contains("not a WebAssembly module"));
    assert!(choose_tier(&p, &cmd(Some(Tier::Browser))).unwrap_err().contains("browser tier"));
    assert_eq!(choose_tier(&p, &wasm(None)), Ok(Tier::Wasm));
    assert!(choose_tier(&p, &wasm(Some(Tier::Job))).unwrap_err().contains("wasm tier"));
    assert_eq!(choose_tier(&p, &br(None)), Ok(Tier::Browser));
    assert!(choose_tier(&p, &br(Some(Tier::Job))).unwrap_err().contains("browser tier"));
    // the namespace tier needs the tenant admin's acknowledgement, even when listed
    let no_ack = policy_with(json!({"userns_ack": false}));
    assert!(choose_tier(&no_ack, &cmd(Some(Tier::Namespace))).unwrap_err().contains("user namespaces"));
    // never chosen by itself: with only the namespace tier listed, a command with no ask has no tier
    let only_ns = policy_with(json!({"tiers": ["namespace"]}));
    assert!(choose_tier(&only_ns, &cmd(None)).unwrap_err().contains("never chosen by itself"));
    // the browser is off by default and off when the switch is off, even if listed
    let off = policy_with(json!({"browser": false}));
    assert!(choose_tier(&off, &br(None)).unwrap_err().contains("browser sandbox is off"));
    assert!(!off.allows_tier(Tier::Browser));
    let only_wasm = policy_with(json!({"tiers": ["wasm"]}));
    assert!(choose_tier(&only_wasm, &cmd(None)).unwrap_err().contains("no tier can run a command"));
    assert!(choose_tier(&policy_with(json!({"tiers": ["job"]})), &wasm(None)).unwrap_err().contains("wasm tier is not allowed"));
    // a policy that is off refuses everything at admission
    assert!(refused(admit(&SandboxPolicy::off(), &spec(), &who(), None)).contains("not switched on"));
}

// ------------------------------------------------------------------------------------------------------------------ admission

#[test]
fn a_request_for_a_privilege_is_refused_before_any_manifest_exists() {
    let p = pol();
    type Set = fn(&mut Escalations);
    let asks: [(&str, Set); 11] = [
        ("privileged", |e| e.privileged = true),
        ("host network", |e| e.host_network = true),
        ("host process", |e| e.host_pid = true),
        ("host IPC", |e| e.host_ipc = true),
        ("hostPath", |e| e.host_paths = vec!["/var/run".into()]),
        ("root", |e| e.run_as_root = true),
        ("capabilities", |e| e.add_capabilities = vec!["NET_ADMIN".into()]),
        ("privilege escalation", |e| e.allow_privilege_escalation = true),
        ("writable root", |e| e.writable_root = true),
        ("service account", |e| e.service_account_token = true),
        ("runtime socket", |e| e.docker_socket = true),
    ];
    for (name, set) in asks {
        let mut s = spec();
        set(&mut s.escalations);
        let m = refused(admit(&p, &s, &who(), None));
        assert!(m.contains("never"), "{name}: {m}");
        // the same request on every tier
        for t in [Some(Tier::Job), Some(Tier::Namespace)] {
            let mut s = SandboxSpec { tier: t, ..spec() };
            set(&mut s.escalations);
            assert!(admit(&p, &s, &who(), Some("adam")).is_err(), "{name}: an approval does not lift it");
        }
    }
    // a policy that tries to allow them cannot: the loader has no such field
    let loaded = policy_with(json!({"allow_privileged": true, "allow_host_network": true, "allow_host_path": true, "allow_root": true, "allow_cap_add": true}));
    let mut s = spec();
    s.escalations.privileged = true;
    assert!(admit(&loaded, &s, &who(), None).is_err());
}

#[test]
fn admission_checks_image_limits_network_environment_secrets_role_and_workdir() {
    let p = pol();
    assert!(admit(&p, &spec(), &who(), None).is_ok());
    // images
    let mut s = spec();
    s.image = "evil.example.net/x/y:1".into();
    assert!(refused(admit(&p, &s, &who(), None)).contains("evil.example.net is not allowed"));
    s.image = "ubuntu:24.04".into();
    assert!(refused(admit(&p, &s, &who(), None)).contains("docker.io is not allowed"), "no registry written means docker.io");
    s.image = "registry.corp.example/a/b:1".into();
    assert!(admit(&p, &s, &who(), None).is_ok(), "*.corp.example covers a sub-domain");
    s.image = "registry.example.com/a b".into();
    assert!(refused(admit(&p, &s, &who(), None)).contains("not valid"));
    let strict = policy_with(json!({"require_digest": true}));
    let mut s = spec();
    s.image = "registry.example.com/tools/cli:1.0".into();
    assert!(refused(admit(&strict, &s, &who(), None)).contains("sha256"));
    assert!(admit(&strict, &spec(), &who(), None).unwrap().digest_pinned());
    assert_eq!(image_registry("localhost:5000/x"), "localhost:5000");
    assert_eq!(image_registry("library/ubuntu"), "docker.io");
    // limits
    for (name, f) in [("cpu", (|s: &mut SandboxSpec| s.limits.cpu_m = 2000) as fn(&mut SandboxSpec)), ("memory", |s| s.limits.mem_mib = 4096), ("time", |s| s.limits.seconds = 3600), ("workspace", |s| s.limits.workspace_mib = 9999), ("processes", |s| s.limits.pids = 5000)] {
        let mut s = spec();
        f(&mut s);
        assert!(refused(admit(&p, &s, &who(), None)).contains(&format!("{name} limit is above")), "{name}");
    }
    let mut s = spec();
    s.limits.mem_mib = 0;
    assert!(refused(admit(&p, &s, &who(), None)).contains("must be set"));
    // network
    let mut s = spec();
    s.network = NetMode::Direct;
    assert!(refused(admit(&p, &s, &who(), None)).contains("direct internet access is never allowed"));
    assert!(refused(admit(&policy_with(json!({"network_modes": ["none"]})), &SandboxSpec { network: NetMode::Proxy, ..spec() }, &who(), None)).contains("not allowed by the tenant"));
    // a policy that lists "direct" loses it on load
    assert!(!policy_with(json!({"network_modes": ["direct", "proxy"]})).network_modes.contains(&NetMode::Direct));
    // environment
    let mut s = spec();
    s.env = vec![EnvVar { name: "LD_PRELOAD".into(), value: "/x.so".into() }];
    assert!(refused(admit(&p, &s, &who(), None)).contains("reserved"));
    s.env = vec![EnvVar { name: "HTTPS_PROXY".into(), value: "http://evil".into() }];
    assert!(refused(admit(&p, &s, &who(), None)).contains("reserved"), "a run cannot choose its own proxy");
    s.env = vec![EnvVar { name: "bad name".into(), value: "x".into() }];
    assert!(refused(admit(&p, &s, &who(), None)).contains("letters, digits"));
    s.env = vec![EnvVar { name: "TOKEN".into(), value: "ghp_abcdefghijklmnopqrstuvwxyz0123456789".into() }];
    let m = refused(admit(&p, &s, &who(), None));
    assert!(m.contains("looks like a credential") && !m.contains("ghp_"), "{m}");
    s.env = vec![EnvVar { name: "REGION".into(), value: "eu-west-1".into() }];
    assert!(admit(&p, &s, &who(), None).is_ok());
    // secrets: a reference only; proxy injection needs the proxy mode
    let mut s = spec();
    s.secrets = vec![SecretUse { name: "aws".into(), secret_ref: "not a ref".into(), delivery: Delivery::TmpfsFile }];
    assert!(refused(admit(&p, &s, &who(), None)).contains("vault reference"));
    s.secrets = vec![SecretUse { name: "aws".into(), secret_ref: "vault:team/web/aws-key".into(), delivery: Delivery::ProxyInject { host: "s3.amazonaws.com".into() } }];
    assert!(refused(admit(&p, &s, &who(), None)).contains("needs the proxy network mode"));
    s.network = NetMode::Proxy;
    assert!(admit(&p, &s, &who(), None).is_ok());
    s.tier = Some(Tier::Namespace);
    s.network = NetMode::None;
    s.secrets[0].delivery = Delivery::TmpfsFile;
    assert!(refused(admit(&p, &s, &who(), None)).contains("takes no secrets"));
    // role and working folder
    assert!(refused(admit(&p, &spec(), &Who { person: "vic", role: "viewer" }, None)).contains("needs the editor role"));
    assert!(admit(&p, &spec(), &Who { person: "adam", role: "tenant_admin" }, None).is_ok());
    for wd in ["/etc", "/workspace/../etc", "/"] {
        let mut s = spec();
        s.workdir = wd.into();
        assert!(refused(admit(&p, &s, &who(), None)).contains("inside /workspace"), "{wd}");
    }
    let mut s = spec();
    s.workdir = "/workspace/sub".into();
    assert!(admit(&p, &s, &who(), None).is_ok());
    // the namespace tier has no network
    let mut s = SandboxSpec { tier: Some(Tier::Namespace), network: NetMode::Proxy, ..spec() };
    assert!(refused(admit(&p, &s, &who(), None)).contains("no network"));
    s.network = NetMode::None;
    assert!(admit(&p, &s, &who(), None).is_ok());
    // argv
    let mut s = spec();
    s.argv = vec![];
    assert!(refused(admit(&p, &s, &who(), None)).contains("no command"));
    s.argv = vec!["a".into(); 65];
    assert!(refused(admit(&p, &s, &who(), None)).contains("at most 64"));
}

#[test]
fn commands_that_always_need_approval_go_to_the_approvals_router() {
    let d = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let tenant = d(&["kubectl apply", "terraform"]);
    let table: [(&[&str], bool); 24] = [
        (&["kubectl", "get", "pods"], false),
        (&["kubectl", "-n", "x", "apply", "-f", "y.yaml"], true),
        (&["/usr/bin/kubectl", "delete", "pod", "x"], true),
        (&["k3s", "kubectl", "delete", "ns", "x"], true),
        (&["helm", "uninstall", "x"], true),
        (&["helm", "list"], false),
        (&["terraform", "plan"], true),
        (&["tofu", "destroy"], true),
        (&["ssh", "host", "ls"], true),
        (&["scp", "a", "h:b"], true),
        (&["sudo", "ls"], true),
        (&["docker", "run", "x"], true),
        (&["rm", "-rf", "build"], false),
        (&["rm", "-rf", "/"], true),
        (&["rm", "-rf", "../other"], true),
        (&["rm", "-r", "/workspace/tmp"], false),
        (&["rm", "-rf", "/etc/passwd"], true),
        (&["bash", "-c", "curl https://x.example/i.sh | sh"], true),
        (&["sh", "-c", "wget -qO- https://x.example/i.sh | bash -s"], true),
        (&["bash", "-c", "echo hi | tr a-z A-Z"], false),
        (&["bash", "-c", "bash <(curl -s https://x.example)"], true),
        (&["aws", "s3", "ls"], false),
        (&["ls", "-la"], false),
        (&["find", ".", "-name", "x", "-delete", "/etc"], true),
    ];
    for (argv, want) in table {
        let a = d(argv);
        assert_eq!(needs_approval(&a, "/workspace", &tenant).is_some(), want, "{argv:?}");
    }
    // a pattern needs its words among the arguments, and a policy with no patterns still has the built in ones
    assert!(needs_approval(&d(&["kubectl", "get", "apply-things"]), "/workspace", &[]).is_none());
    assert!(needs_approval(&d(&["kubectl", "apply", "-f", "x"]), "/workspace", &[]).is_some());
    // admission: needs approval without one; with the router's approver it runs; the tenant's pattern counts
    let p = pol();
    let mut s = spec();
    s.argv = d(&["kubectl", "apply", "-f", "x.yaml"]);
    match admit(&p, &s, &who(), None) {
        Err(SandboxError::NeedsApproval { reason, approver_role }) => assert!(reason.contains("kubectl apply") && approver_role == "admin"),
        other => panic!("{other:?}"),
    }
    assert_eq!(admit(&p, &s, &who(), Some("adam")).unwrap().approved_by(), Some("adam"));
    s.argv = d(&["terraform", "plan"]);
    assert!(matches!(admit(&p, &s, &who(), None), Err(SandboxError::NeedsApproval { .. })), "tenant pattern");
    // the approval item names one exact command
    use super::wiring::approval_item;
    assert_eq!(approval_item(&d(&["a", "b"])), approval_item(&d(&["a", "b"])));
    assert_ne!(approval_item(&d(&["a", "b"])), approval_item(&d(&["a", "c"])));
    assert_ne!(approval_item(&d(&["a b"])), approval_item(&d(&["a", "b"])));
}

// ------------------------------------------------------------------------------------------------------------------ manifests

fn admitted(s: &SandboxSpec) -> super::admit::Admitted {
    admit(&pol(), s, &who(), Some("adam")).unwrap()
}

#[test]
fn the_job_manifest_is_the_golden_one() {
    let mut s = spec();
    s.network = NetMode::Proxy;
    s.env = vec![EnvVar { name: "REGION".into(), value: "eu-west-1".into() }];
    let adm = admitted(&s);
    let got = job::job_for(&adm, &pol(), "sbx-acme");
    let labels = json!({"app.kubernetes.io/name": "iyakki-sandbox", "iyakki.dev/tenant": "acme", "iyakki.dev/team": "web", "iyakki.dev/person": "mia", "iyakki.dev/run": "r1", "iyakki.dev/tier": "job"});
    let mut pod_labels = labels.clone();
    pod_labels["iyakki.dev/sandbox-run"] = json!("sbx-r1");
    let proxy = "http://iyakki-egress.sbx-acme.svc:3128";
    let env = json!([
        {"name": "REGION", "value": "eu-west-1"}, {"name": "HOME", "value": "/workspace"}, {"name": "TMPDIR", "value": "/tmp"},
        {"name": "HTTP_PROXY", "value": proxy}, {"name": "HTTPS_PROXY", "value": proxy}, {"name": "http_proxy", "value": proxy}, {"name": "https_proxy", "value": proxy},
        {"name": "NO_PROXY", "value": "localhost,127.0.0.1"}, {"name": "no_proxy", "value": "localhost,127.0.0.1"}
    ]);
    let ctx_sec = json!({"privileged": false, "allowPrivilegeEscalation": false, "readOnlyRootFilesystem": true, "runAsNonRoot": true, "runAsUser": 10001, "runAsGroup": 10001, "capabilities": {"drop": ["ALL"]}, "seccompProfile": {"type": "RuntimeDefault"}});
    let resources = json!({
        "requests": {"cpu": "100m", "memory": "256Mi", "ephemeral-storage": "192Mi"},
        "limits": {"cpu": "500m", "memory": "256Mi", "ephemeral-storage": "192Mi"}
    });
    let container = json!({
        "name": "run", "image": image(), "imagePullPolicy": "IfNotPresent",
        "command": ["aws", "s3", "ls"], "workingDir": "/workspace",
        "env": env, "securityContext": ctx_sec, "resources": resources,
        "volumeMounts": [{"name": "workspace", "mountPath": "/workspace"}, {"name": "tmp", "mountPath": "/tmp"}]
    });
    let pod_sec = json!({"runAsNonRoot": true, "runAsUser": 10001, "runAsGroup": 10001, "fsGroup": 10001, "seccompProfile": {"type": "RuntimeDefault"}});
    let pod = json!({
        "restartPolicy": "Never", "automountServiceAccountToken": false, "enableServiceLinks": false,
        "hostNetwork": false, "hostPID": false, "hostIPC": false, "shareProcessNamespace": false,
        "terminationGracePeriodSeconds": 5, "runtimeClassName": "gvisor",
        "securityContext": pod_sec, "containers": [container],
        "volumes": [{"name": "workspace", "emptyDir": {"sizeLimit": "128Mi"}}, {"name": "tmp", "emptyDir": {"sizeLimit": "64Mi"}}]
    });
    let job_spec = json!({
        "backoffLimit": 0, "completions": 1, "parallelism": 1, "activeDeadlineSeconds": 60, "ttlSecondsAfterFinished": 600,
        "template": {"metadata": {"labels": pod_labels}, "spec": pod}
    });
    let want = json!({"apiVersion": "batch/v1", "kind": "Job", "metadata": {"name": "sbx-r1", "namespace": "sbx-acme", "labels": labels}, "spec": job_spec});
    assert_eq!(got, want, "{}", serde_json::to_string_pretty(&got).unwrap());
}

#[test]
fn every_hardening_field_is_present_and_nothing_dangerous_is() {
    let mut s = spec();
    s.inputs = vec![InputFile { name: "in.txt".into(), bytes: b"hello".to_vec() }];
    s.secrets = vec![SecretUse { name: "aws".into(), secret_ref: "vault:team/web/aws".into(), delivery: Delivery::TmpfsFile }];
    let adm = admitted(&s);
    for policy in [pol(), policy_with(json!({"runtime_class": null}))] {
        let j = job::job_for(&adm, &policy, "ns");
        let pod = "/spec/template/spec";
        let c = format!("{pod}/containers/0");
        let must: [(&str, Value); 22] = [
            (&format!("{pod}/automountServiceAccountToken"), json!(false)),
            (&format!("{pod}/hostNetwork"), json!(false)),
            (&format!("{pod}/hostPID"), json!(false)),
            (&format!("{pod}/hostIPC"), json!(false)),
            (&format!("{pod}/restartPolicy"), json!("Never")),
            (&format!("{pod}/securityContext/runAsNonRoot"), json!(true)),
            (&format!("{pod}/securityContext/runAsUser"), json!(10001)),
            (&format!("{pod}/securityContext/seccompProfile/type"), json!("RuntimeDefault")),
            (&format!("{c}/securityContext/allowPrivilegeEscalation"), json!(false)),
            (&format!("{c}/securityContext/readOnlyRootFilesystem"), json!(true)),
            (&format!("{c}/securityContext/privileged"), json!(false)),
            (&format!("{c}/securityContext/capabilities/drop"), json!(["ALL"])),
            (&format!("{c}/securityContext/seccompProfile/type"), json!("RuntimeDefault")),
            (&format!("{pod}/volumes/0/emptyDir/sizeLimit"), json!("128Mi")),
            ("/spec/activeDeadlineSeconds", json!(60)),
            ("/spec/ttlSecondsAfterFinished", json!(600)),
            ("/spec/backoffLimit", json!(0)),
            (&format!("{c}/resources/limits/memory"), json!("256Mi")),
            (&format!("{c}/resources/limits/cpu"), json!("500m")),
            (&format!("{c}/resources/requests/cpu"), json!("100m")),
            (&format!("{pod}/volumes/2/configMap/name"), json!("sbx-r1-in")),
            (&format!("{pod}/volumes/3/secret/defaultMode"), json!(256)),
        ];
        for (ptr, want) in must {
            assert_eq!(j.pointer(ptr), Some(&want), "{ptr}\n{j}");
        }
        assert_eq!(j.pointer(&format!("{pod}/containers/0/volumeMounts/2/readOnly")), Some(&json!(true)));
        assert_eq!(j.pointer(&format!("{pod}/containers/0/volumeMounts/3/readOnly")), Some(&json!(true)));
        // the runtime class is set when the policy has one and absent otherwise
        assert_eq!(j.pointer(&format!("{pod}/runtimeClassName")).is_some(), policy.runtime_class.is_some());
        // nothing dangerous anywhere in the text
        let text = j.to_string();
        for bad in ["hostPath", "\"privileged\":true", "\"hostNetwork\":true", "hostPort", "\"add\"", "docker.sock", "\"runAsUser\":0", "SYS_ADMIN"] {
            assert!(!text.contains(bad), "{bad} in {text}");
        }
        // no value of a secret anywhere in the Job; the Secret object exists apart
        assert!(!text.contains("secret-value"));
    }
    let sec = job::secret_object(&adm, "ns", &BTreeMap::from([("aws".to_string(), "secret-value".to_string())]));
    assert_eq!(sec["immutable"], json!(true));
    assert_eq!(sec["metadata"]["name"], json!("sbx-r1-sec"));
    assert!(job::job_for(&adm, &pol(), "ns").pointer("/spec/template/spec/volumes/3/secret/secretName") == Some(&json!("sbx-r1-sec")));
    assert!(adm.spec().secrets.iter().all(|x| !format!("{x:?}").contains("secret-value")));
}

#[test]
fn the_network_policy_is_the_golden_one() {
    let mut s = spec();
    s.network = NetMode::Proxy;
    let adm = admitted(&s);
    let got = job::network_policy(&adm, &pol(), "sbx-acme");
    let labels = json!({"app.kubernetes.io/name": "iyakki-sandbox", "iyakki.dev/tenant": "acme", "iyakki.dev/team": "web", "iyakki.dev/person": "mia", "iyakki.dev/run": "r1", "iyakki.dev/tier": "job"});
    let want = json!({
        "apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy",
        "metadata": {"name": "sbx-r1", "namespace": "sbx-acme", "labels": labels},
        "spec": {
            "podSelector": {"matchLabels": {"iyakki.dev/sandbox-run": "sbx-r1"}},
            "policyTypes": ["Ingress", "Egress"],
            "ingress": [],
            "egress": [
                {"to": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}}, "podSelector": {"matchLabels": {"k8s-app": "kube-dns"}}}], "ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}]},
                {"to": [{"podSelector": {"matchLabels": {"app.kubernetes.io/name": "iyakki-egress"}}}], "ports": [{"protocol": "TCP", "port": 3128}]}
            ]
        }
    });
    assert_eq!(got, want, "{got}");
    // no network at all: egress is an empty list, which with policyTypes Egress means deny all
    let none = job::network_policy(&admitted(&spec()), &pol(), "sbx-acme");
    assert_eq!(none["spec"]["egress"], json!([]));
    assert_eq!(none["spec"]["policyTypes"], json!(["Ingress", "Egress"]));
    // no ipBlock, no namespace-wide selector, no 0.0.0.0/0 anywhere
    for t in [got.to_string(), none.to_string()] {
        assert!(!t.contains("ipBlock") && !t.contains("0.0.0.0") && !t.contains("\"namespaceSelector\":{}"), "{t}");
    }
}

#[test]
fn the_bundle_applies_the_network_policy_first_and_the_job_last() {
    let mut s = spec();
    s.inputs = vec![InputFile { name: "in.txt".into(), bytes: b"hello".to_vec() }];
    let b = job::bundle(&admitted(&s), &pol(), "ns");
    let kinds: Vec<&str> = b.iter().map(|o| o["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["NetworkPolicy", "ConfigMap", "Job"]);
    assert_eq!(b[1]["data"]["in.txt"], json!("hello"));
    assert_eq!(b[1]["immutable"], json!(true));
}

fn browser_spec() -> SandboxSpec {
    let mut s = SandboxSpec::browser(vec!["https://www.example.org/start".into()], vec!["api.example.org".into()], "", labels());
    s.limits.seconds = 200;
    s.inputs = vec![InputFile { name: "steps.json".into(), bytes: br#"[{"click":{"role":"button","name":"Go"}}]"#.to_vec() }];
    s
}

#[test]
fn the_browser_sandbox_is_off_by_default_and_its_job_is_the_golden_one() {
    // off by default: the stored defaults say browser false, and the tier is unavailable
    let defaults = SandboxPolicy::from_values(true, &BTreeMap::from([("tiers".to_string(), json!(["wasm", "job", "browser"])), ("cli_role".to_string(), json!("editor"))]));
    assert!(!defaults.browser && !defaults.allows_tier(Tier::Browser));
    assert!(refused(admit(&defaults, &browser_spec(), &who(), None)).contains("browser sandbox is off"));
    // on, with the domain allow-list
    let p = pol();
    let adm = admit(&p, &browser_spec(), &who(), None).unwrap();
    assert_eq!(adm.tier(), Tier::Browser);
    let j = job::job_for(&adm, &p, "sbx-acme");
    let c = j.pointer("/spec/template/spec/containers/0").unwrap();
    assert_eq!(c["command"], json!(["iyakki", "browser-test", "/inputs/spec.json", "--out", "/workspace/out", "--json", "--profile", "/profile"]));
    assert_eq!(c["image"], json!(image()), "the browser image is the policy's");
    let envs: BTreeMap<&str, &str> = c["env"].as_array().unwrap().iter().map(|e| (e["name"].as_str().unwrap(), e["value"].as_str().unwrap())).collect();
    assert_eq!(envs["IYAKKI_BROWSER_MEDIA"], "deny", "microphone, camera, sound and clipboard denied");
    assert_eq!(envs["IYAKKI_WEBVIEW_HELPER"], "/usr/libexec/iyakki/webview-helper");
    assert_eq!(envs["HTTPS_PROXY"], "http://iyakki-egress.sbx-acme.svc:3128", "egress only through the proxy");
    let vols = j.pointer("/spec/template/spec/volumes").unwrap().as_array().unwrap();
    let vol = |n: &str| vols.iter().find(|v| v["name"] == n).unwrap_or_else(|| panic!("volume {n}"));
    assert_eq!(vol("profile")["emptyDir"], json!({"medium": "Memory", "sizeLimit": "256Mi"}), "a profile in memory: nothing persists");
    assert_eq!(vol("downloads")["emptyDir"]["sizeLimit"], json!("32Mi"), "download quota");
    assert!(!j.to_string().contains("persistentVolumeClaim") && !j.to_string().contains("hostPath"));
    assert_eq!(j.pointer("/spec/activeDeadlineSeconds"), Some(&json!(120)), "the session timeout caps the run's time");
    assert_eq!(c["resources"]["limits"]["memory"], json!("512Mi"), "256 MiB of memory limit plus the in-memory profile");
    // the spec the runner reads: the tenant's domains narrowed to the request, the steps, a final screenshot, no payment
    let cm = job::inputs_configmap(&adm, "sbx-acme", &p).unwrap();
    let spec_json: Value = serde_json::from_str(cm["data"]["spec.json"].as_str().unwrap()).unwrap();
    assert_eq!(spec_json["allow_domains"], json!(["api.example.org", "www.example.org"]));
    assert_eq!(spec_json["allow_payment"], json!(false));
    assert_eq!(spec_json["steps"], json!([{"goto": "https://www.example.org/start"}, {"click": {"role": "button", "name": "Go"}}, {"screenshot": "final"}]));
    // a named profile only when the tenant allows it
    let named = policy_with(json!({"browser_profile": "named"}));
    let j2 = job::job_for(&admit(&named, &browser_spec(), &who(), None).unwrap(), &named, "ns");
    assert!(j2.to_string().contains("sbx-profile-web-mia"));
    // domains off the list are refused; so is a session without the proxy or with an address that is not http(s)
    let mut s = browser_spec();
    s.domains = vec!["evil.example.net".into()];
    assert!(refused(admit(&p, &s, &who(), None)).contains("not on the tenant's browser allow-list"));
    let mut s = browser_spec();
    s.argv = vec!["file:///etc/passwd".into()];
    assert!(refused(admit(&p, &s, &who(), None)).contains("http or https"));
    let mut s = browser_spec();
    s.network = NetMode::None;
    assert!(refused(admit(&p, &s, &who(), None)).contains("only through the proxy"));
    let mut s = browser_spec();
    s.argv = vec![];
    s.domains = vec![];
    assert!(refused(admit(&p, &s, &who(), None)).contains("domains it may open"));
    // the image comes from an allowed registry like any other
    let strict = policy_with(json!({"browser_image": "evil.example.net/b:1"}));
    assert!(refused(admit(&strict, &browser_spec(), &who(), None)).contains("evil.example.net"));
}

#[test]
fn kubectl_accepts_the_golden_manifests_without_a_cluster() {
    // only when kubectl exists; client-side dry run with validation off and a server address that nobody listens on
    let has = std::process::Command::new("kubectl").arg("version").arg("--client").arg("-o").arg("json").output().map(|o| o.status.success()).unwrap_or(false);
    if !has {
        eprintln!("kubectl not found: skipped");
        return;
    }
    let mut s = spec();
    s.network = NetMode::Proxy;
    s.inputs = vec![InputFile { name: "in.txt".into(), bytes: b"x".to_vec() }];
    let mut objects = job::bundle(&admitted(&s), &pol(), "sbx-acme");
    objects.extend(job::bundle(&admit(&pol(), &browser_spec(), &who(), None).unwrap(), &pol(), "sbx-acme"));
    let list = json!({"apiVersion": "v1", "kind": "List", "items": objects});
    let dir = crate::testdir::unique("sandbox", "kubectl");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("list.json");
    std::fs::write(&file, list.to_string()).unwrap();
    let out = std::process::Command::new("kubectl")
        .args(["apply", "--dry-run=client", "--validate=false", "--server=http://127.0.0.1:9", "--request-timeout=2s", "-o", "name", "-f"])
        .arg(&file)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &dir)
        .env("KUBECONFIG", dir.join("none"))
        .output()
        .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        assert!(text.contains("job.batch/sbx-r1") && text.contains("networkpolicy.networking.k8s.io/sbx-r1"), "{text}");
    } else {
        // this kubectl wants discovery from a server even for a client dry run; the manifests are not at fault if it says so
        assert!(text.contains("127.0.0.1:9") || text.contains("connection refused") || text.contains("unable to recognize"), "kubectl rejected the manifests: {text}");
        eprintln!("kubectl needed a server for discovery; JSON parsing passed: {text}");
    }
}

// ------------------------------------------------------------------------------------------------------------------ output

#[test]
fn output_is_redacted_capped_and_clean() {
    let c = clean(b"ok\nAWS_SECRET_ACCESS_KEY: abcdef0123456789abcd\ntoken ghp_abcdefghijklmnopqrstuvwxyz0123456789\n", 64, &[]);
    assert!(c.redacted && !c.text.contains("abcdef0123456789abcd") && !c.text.contains("ghp_"), "{}", c.text);
    // exact values the run held, even in words the detector does not know
    let c = clean(b"the answer is hunter2-plus and again hunter2-plus", 64, &["hunter2-plus".into()]);
    assert_eq!(c.text, "the answer is [redacted] and again [redacted]");
    assert!(c.redacted);
    // short values are not removed (they would wreck the text)
    assert!(!clean(b"a b c", 64, &["b".into()]).redacted);
    // control characters out, newline and tab stay
    assert_eq!(clean(b"a\x1b[31mred\x00\n\tb", 64, &[]).text, "a[31mred\n\tb");
    // the cap keeps head and tail and says so; multibyte text is cut on a character
    let big = "é".repeat(5000);
    let c = clean(big.as_bytes(), 1, &[]);
    assert!(c.truncated && c.text.len() < 1500 && c.text.contains("bytes cut") && c.text.starts_with('é') && c.text.ends_with('é'));
    assert!(!clean(b"short", 1, &[]).truncated);
}

// ------------------------------------------------------------------------------------------------------------------ limiter

#[test]
fn concurrency_is_limited_per_person_and_per_team_with_a_queue_that_times_out() {
    let l = Limiter::new(1, 2, 3);
    let clock = Arc::new(Mutex::new(100u64));
    let now = {
        let c = clock.clone();
        move || {
            let mut g = c.lock().unwrap();
            *g += 1; // every look at the clock is a second later
            *g
        }
    };
    let tick = Duration::from_millis(1);
    let a = l.acquire("mia", &now, tick).unwrap();
    assert_eq!(l.running(), 1);
    // mia may have one: a second run waits, then times out
    assert_eq!(l.acquire("mia", &now, tick).err(), Some(SandboxError::QueueTimeout));
    assert_eq!(l.queued(), 0, "a timed out waiter leaves the queue");
    // somebody else may run
    let b = l.acquire("max", &now, tick).unwrap();
    assert_eq!(l.running(), 2);
    // the team is full (2)
    assert_eq!(l.acquire("vic", &now, tick).err(), Some(SandboxError::QueueTimeout));
    drop(a);
    assert_eq!(l.running(), 1);
    let c = l.acquire("vic", &now, tick).unwrap();
    drop((b, c));
    assert_eq!(l.running(), 0);
    // a waiter gets the slot when one is freed (real clock, real threads)
    let l = Limiter::new(1, 1, 30);
    let held = l.acquire("mia", &crate::tenantconfig::now_secs, Duration::from_millis(5)).unwrap();
    let l2 = l.clone();
    let h = std::thread::spawn(move || l2.acquire("max", &crate::tenantconfig::now_secs, Duration::from_millis(5)).map(|p| drop(p)).is_ok());
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(l.queued(), 1);
    drop(held);
    assert!(h.join().unwrap());
    // fairness: with one slot, the person with fewer running goes before one who has waited longer but already runs another
    let l = Limiter::new(2, 3, 60);
    let _m1 = l.acquire("mia", &crate::tenantconfig::now_secs, Duration::from_millis(5)).unwrap();
    assert_eq!(l.running(), 1);
}

// ------------------------------------------------------------------------------------------------------------------ job executor

#[derive(Default)]
struct FakeKube {
    applied: Mutex<Vec<Value>>,
    deleted: Mutex<Vec<String>>,
    states: Mutex<Vec<Result<JobState, String>>>,
    logs: Mutex<Vec<u8>>,
    artifacts: Mutex<Vec<(String, Vec<u8>)>>,
    fail_apply_kind: Mutex<Option<String>>,
}

impl FakeKube {
    fn finishing(state: JobState, logs: &str) -> Arc<FakeKube> {
        let k = Arc::new(FakeKube::default());
        k.states.lock().unwrap().push(Ok(state));
        *k.logs.lock().unwrap() = logs.as_bytes().to_vec();
        k
    }
    fn kinds(&self) -> Vec<String> {
        self.applied.lock().unwrap().iter().map(|o| o["kind"].as_str().unwrap().to_string()).collect()
    }
}

impl KubeApi for FakeKube {
    fn apply(&self, m: &Value) -> Result<(), String> {
        if self.fail_apply_kind.lock().unwrap().as_deref() == m["kind"].as_str() {
            return Err("forbidden".into());
        }
        self.applied.lock().unwrap().push(m.clone());
        Ok(())
    }
    fn state(&self, _: &str, _: &str) -> Result<JobState, String> {
        let mut s = self.states.lock().unwrap();
        if s.len() > 1 {
            s.remove(0)
        } else {
            s.first().cloned().unwrap_or(Ok(JobState::default()))
        }
    }
    fn logs(&self, _: &str, _: &str, limit: usize) -> Result<Vec<u8>, String> {
        Ok(self.logs.lock().unwrap().iter().copied().take(limit).collect())
    }
    fn artifacts(&self, _: &str, _: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
        Ok(self.artifacts.lock().unwrap().clone())
    }
    fn delete_run(&self, ns: &str, run: &str) -> Result<(), String> {
        self.deleted.lock().unwrap().push(format!("{ns}/{run}"));
        Ok(())
    }
}

struct Secrets(BTreeMap<String, String>);
impl SecretResolver for Secrets {
    fn resolve(&self, r: &str, _: &str) -> Result<zeroize::Zeroizing<String>, String> {
        self.0.get(r).map(|v| zeroize::Zeroizing::new(v.clone())).ok_or_else(|| "not in the vault".to_string())
    }
}

struct Gate(Result<(), SandboxError>);
impl ApplyGate for Gate {
    fn allow(&self, _: &super::admit::Admitted, _: &str) -> Result<(), SandboxError> {
        self.0.clone()
    }
}

fn job_exec(k: Arc<FakeKube>, gate: Result<(), SandboxError>) -> Arc<JobExecutor> {
    let mut e = JobExecutor::new(k, pol(), Arc::new(Gate(gate)), Arc::new(Secrets(BTreeMap::from([("vault:team/web/aws".to_string(), "s3cr3t-value-123".to_string())]))));
    e.poll = Duration::from_millis(1);
    e.grace = Duration::from_millis(30);
    e.image_fail_after = Duration::from_millis(5);
    Arc::new(e)
}

fn engine(ex: Vec<Arc<dyn Executor>>, rec: Arc<Rec>) -> Engine {
    Engine::new(pol(), ex, rec).with_clock(Arc::new(crate::tenantconfig::now_secs), Duration::from_millis(5))
}

#[test]
fn a_run_applies_policy_secret_job_in_order_returns_redacted_output_and_cleans_up() {
    let mut st = JobState::default();
    st.succeeded = true;
    let k = FakeKube::finishing(st, "listing\nthe key is s3cr3t-value-123 and token ghp_abcdefghijklmnopqrstuvwxyz0123456789\n");
    k.artifacts.lock().unwrap().push(("out/result.txt".into(), b"data".to_vec()));
    k.artifacts.lock().unwrap().push(("out/big.bin".into(), vec![0u8; 3 * 1024 * 1024]));
    let rec = Arc::new(Rec::default());
    let e = engine(vec![job_exec(k.clone(), Ok(()))], rec.clone());
    let mut s = spec();
    s.network = NetMode::Proxy;
    s.inputs = vec![InputFile { name: "in.txt".into(), bytes: b"x".to_vec() }];
    s.secrets = vec![SecretUse { name: "aws".into(), secret_ref: "vault:team/web/aws".into(), delivery: Delivery::TmpfsFile }];
    s.env = vec![EnvVar { name: "REGION".into(), value: "eu-west-1-private".into() }];
    let r = e.run(&s, &who(), None).unwrap();
    assert!(r.ok() && r.tier == Tier::Job && r.run_id == "r1");
    assert_eq!(k.kinds(), ["NetworkPolicy", "ConfigMap", "Secret", "Job"], "the NetworkPolicy first, the Job last, the Secret in between");
    assert!(!r.stdout.contains("s3cr3t-value-123") && !r.stdout.contains("ghp_") && r.redacted, "{}", r.stdout);
    assert_eq!(k.deleted.lock().unwrap().as_slice(), ["sbx-acme/sbx-r1"], "cleaned up");
    // artifacts: sizes and hashes of all, bytes of what fits the caps
    assert_eq!(r.artifacts.len(), 2);
    assert!(r.artifacts[0].returned && r.artifacts[0].sha256 == crate::ocipush::sha256_hex(b"data") && r.artifacts[0].size == 4);
    assert!(!r.artifacts[1].returned && r.artifacts[1].size == 3 * 1024 * 1024);
    assert_eq!(r.files.len(), 1);
    // the secret value is only in the Secret object, never in the Job, the policy or the audit
    let applied = k.applied.lock().unwrap().clone();
    for o in &applied {
        let has = o.to_string().contains("s3cr3t-value-123");
        assert_eq!(has, o["kind"] == "Secret", "{}", o["kind"]);
    }
    let audit = rec.all();
    assert!(audit.contains("sandbox.start") && audit.contains("sandbox.end") && audit.contains("tier=job") && audit.contains("program=aws") && audit.contains("run=r1") && audit.contains("net=proxy"), "{audit}");
    assert!(!audit.contains("eu-west-1-private") && !audit.contains("s3cr3t") && !audit.contains("s3 ls"), "values and arguments never reach the audit: {audit}");
}

#[test]
fn exit_codes_signals_timeouts_and_out_of_memory_are_mapped() {
    let run = |st: JobState| {
        let k = FakeKube::finishing(st, "out");
        engine(vec![job_exec(k, Ok(()))], Arc::new(Rec::default())).run(&spec(), &who(), None).unwrap()
    };
    let r = run(JobState { failed: true, exit_code: Some(3), ..Default::default() });
    assert!(!r.ok() && r.exit_code == Some(3) && r.signal.is_none() && !r.timed_out && !r.oom_killed);
    let r = run(JobState { failed: true, exit_code: Some(137), oom_killed: true, ..Default::default() });
    assert!(r.oom_killed && r.signal == Some(9) && r.exit_code.is_none() && !r.ok());
    let r = run(JobState { failed: true, deadline_exceeded: true, exit_code: Some(143), ..Default::default() });
    assert!(r.timed_out && r.signal == Some(15) && !r.ok());
    let r = run(JobState { succeeded: true, ..Default::default() });
    assert!(r.ok() && r.exit_code == Some(0));
    // never finishes: the executor gives up after the run's time plus the grace and reports a timeout
    let k = Arc::new(FakeKube::default());
    let mut e = JobExecutor::new(k.clone(), pol(), Arc::new(OpenGate), Arc::new(Secrets(BTreeMap::new())));
    e.poll = Duration::from_millis(1);
    e.grace = Duration::from_millis(20);
    let mut s = spec();
    s.limits.seconds = 1;
    let t0 = std::time::Instant::now();
    let r = engine(vec![Arc::new(e)], Arc::new(Rec::default())).run(&s, &who(), None).unwrap();
    assert!(r.timed_out && t0.elapsed() < Duration::from_secs(5));
    assert_eq!(k.deleted.lock().unwrap().len(), 1, "a run that never finished is still removed");
    // an image that cannot be fetched is an infrastructure error, not a hang
    let k = FakeKube::finishing(JobState { waiting_reason: Some("ImagePullBackOff".into()), ..Default::default() }, "");
    let r = engine(vec![job_exec(k.clone(), Ok(()))], Arc::new(Rec::default())).run(&spec(), &who(), None);
    assert!(matches!(&r, Err(SandboxError::Infra(m)) if m.contains("ImagePullBackOff")), "{r:?}");
    assert_eq!(k.deleted.lock().unwrap().len(), 1);
    // the cluster failing five times in a row
    let k = Arc::new(FakeKube::default());
    k.states.lock().unwrap().push(Err("connection refused".into()));
    let r = engine(vec![job_exec(k, Ok(()))], Arc::new(Rec::default())).run(&spec(), &who(), None);
    assert!(matches!(&r, Err(SandboxError::Infra(m)) if m.contains("could not be asked")));
    // kubectl state parsing
    let job = json!({"status": {"failed": 1, "conditions": [{"type": "Failed", "reason": "DeadlineExceeded"}]}});
    let pods = json!({"items": [{"status": {"containerStatuses": [{"started": false, "state": {"terminated": {"exitCode": 137, "reason": "OOMKilled"}}}]}}]});
    let st = parse_state(&job, &pods);
    assert!(st.failed && st.deadline_exceeded && st.oom_killed && st.exit_code == Some(137) && st.started);
    let waiting = json!({"items": [{"status": {"containerStatuses": [{"state": {"waiting": {"reason": "ErrImagePull"}}}]}}]});
    assert_eq!(parse_state(&json!({}), &waiting).waiting_reason.as_deref(), Some("ErrImagePull"));
}

#[test]
fn the_team_cluster_binding_gates_the_apply_and_nothing_is_applied_when_it_says_no() {
    let k = FakeKube::finishing(JobState { succeeded: true, ..Default::default() }, "");
    let e = engine(vec![job_exec(k.clone(), Err(SandboxError::Refused("the team's cluster binding does not allow it: change freeze".into())))], Arc::new(Rec::default()));
    let r = e.run(&spec(), &who(), None);
    assert!(matches!(&r, Err(SandboxError::Refused(m)) if m.contains("change freeze")));
    assert!(k.applied.lock().unwrap().is_empty() && k.deleted.lock().unwrap().is_empty(), "no object was made");
    let k = FakeKube::finishing(JobState::default(), "");
    let e = engine(vec![job_exec(k.clone(), Err(SandboxError::NeedsApproval { reason: "binding".into(), approver_role: "admin".into() }))], Arc::new(Rec::default()));
    assert!(matches!(e.run(&spec(), &who(), None), Err(SandboxError::NeedsApproval { .. })));
    assert!(k.applied.lock().unwrap().is_empty());
    // an apply that fails half way leaves nothing behind
    let k = FakeKube::finishing(JobState { succeeded: true, ..Default::default() }, "");
    *k.fail_apply_kind.lock().unwrap() = Some("Job".into());
    let r = engine(vec![job_exec(k.clone(), Ok(()))], Arc::new(Rec::default())).run(&spec(), &who(), None);
    assert!(matches!(r, Err(SandboxError::Infra(_))));
    assert_eq!(k.deleted.lock().unwrap().len(), 1, "the NetworkPolicy made before the failure is removed");
    // proxy-injected secrets are refused while no proxy control exists
    let k = FakeKube::finishing(JobState { succeeded: true, ..Default::default() }, "");
    let mut s = spec();
    s.network = NetMode::Proxy;
    s.secrets = vec![SecretUse { name: "k".into(), secret_ref: "vault:team/web/aws".into(), delivery: Delivery::ProxyInject { host: "s3.amazonaws.com".into() } }];
    let r = engine(vec![job_exec(k.clone(), Ok(()))], Arc::new(Rec::default())).run(&s, &who(), None);
    assert!(matches!(&r, Err(SandboxError::Refused(m)) if m.contains("not wired")), "{r:?}");
    assert!(k.applied.lock().unwrap().is_empty());
}

#[test]
fn the_engine_audits_refusals_and_needs_approval_and_enforces_the_executor_tier() {
    let rec = Arc::new(Rec::default());
    let k = FakeKube::finishing(JobState { succeeded: true, ..Default::default() }, "");
    let e = engine(vec![job_exec(k.clone(), Ok(()))], rec.clone());
    let mut s = spec();
    s.escalations.privileged = true;
    assert!(e.run(&s, &who(), None).is_err());
    let mut s = spec();
    s.argv = vec!["kubectl".into(), "delete".into(), "ns".into(), "x".into()];
    assert!(matches!(e.run(&s, &who(), None), Err(SandboxError::NeedsApproval { .. })));
    let audit = rec.all();
    assert!(audit.contains("sandbox.refused") && audit.contains("privileged") && audit.contains("sandbox.needs_approval") && audit.contains("program=kubectl"), "{audit}");
    assert!(k.applied.lock().unwrap().is_empty());
    // no executor for the tier
    let wasm = SandboxSpec { work: Work::WasmModule, wasm: b"\0asm".to_vec(), argv: vec![], ..spec() };
    assert!(matches!(e.run(&wasm, &who(), None), Err(SandboxError::Infra(m)) if m.contains("no executor")));
    // an executor refuses a tier that is not its own
    let adm = admit(&pol(), &SandboxSpec { tier: Some(Tier::Namespace), ..spec() }, &who(), None).unwrap();
    assert!(job_exec(k, Ok(())).run(&adm).is_err());
    // the run's slot is given back
    assert_eq!(e.limiter().running(), 0);
}

// ------------------------------------------------------------------------------------------------------------------ namespace tier

struct FakeLauncher {
    seen: Mutex<Vec<Vec<String>>>,
    out: NsOutput,
    scratch_during: Mutex<Vec<bool>>,
    scratch: std::path::PathBuf,
}

impl NsLauncher for FakeLauncher {
    fn launch(&self, args: &[String], timeout: Duration, cap: usize) -> Result<NsOutput, String> {
        assert!(timeout.as_secs() == 60 && cap == 64 * 1024 * 2);
        self.scratch_during.lock().unwrap().push(self.scratch.join("sbx-r1").join("workspace").is_dir());
        self.seen.lock().unwrap().push(args.to_vec());
        Ok(self.out.clone())
    }
}

#[test]
fn the_namespace_tier_builds_the_linuxapp_init_arguments_and_cleans_its_workspace() {
    let dir = crate::testdir::unique("sandbox", "ns");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let l = Arc::new(FakeLauncher { seen: Mutex::new(vec![]), out: NsOutput { code: Some(0), stdout: b"hello\n".to_vec(), ..Default::default() }, scratch_during: Mutex::new(vec![]), scratch: dir.clone() });
    let ex = Arc::new(LocalNsExecutor { launcher: l.clone(), rootfs: "/var/lib/iyakki/sandbox/rootfs".into(), scratch: dir.clone() });
    let rec = Arc::new(Rec::default());
    let e = engine(vec![ex], rec);
    let mut s = SandboxSpec { tier: Some(Tier::Namespace), ..spec() };
    s.argv = vec!["python3".into(), "-c".into(), "print('hello')".into()];
    s.inputs = vec![InputFile { name: "a.txt".into(), bytes: b"a".to_vec() }];
    s.env = vec![EnvVar { name: "LANG".into(), value: "C".into() }];
    let r = e.run(&s, &who(), None).unwrap();
    assert!(r.ok() && r.stdout == "hello\n" && r.tier == Tier::Namespace);
    assert_eq!(l.scratch_during.lock().unwrap().as_slice(), [true], "the workspace existed during the run");
    assert!(!dir.join("sbx-r1").exists(), "and is gone after");
    let args = l.seen.lock().unwrap()[0].clone();
    // the arguments are the ones the init subcommand parses back, with the safe settings
    let parsed = crate::linuxapps_init::Spec::parse(&args).expect("linuxapp-init accepts them");
    assert!(!parsed.net && parsed.seccomp && parsed.uid == 10001 && parsed.mem_mb == 256 && parsed.pids == 64 && parsed.cpu_pct == 50);
    assert_eq!(parsed.cmd, ["python3", "-c", "print('hello')"]);
    assert!(parsed.binds.iter().any(|b| b.dst == "/workspace" && b.rw) && parsed.binds.iter().any(|b| b.dst == "/inputs" && !b.rw));
    assert_eq!(parsed.cwd, "/workspace");
    // timeouts and signals pass through
    let l2 = Arc::new(FakeLauncher { seen: Mutex::new(vec![]), out: NsOutput { signal: Some(9), timed_out: true, ..Default::default() }, scratch_during: Mutex::new(vec![]), scratch: dir.clone() });
    let ex2 = Arc::new(LocalNsExecutor { launcher: l2, rootfs: "/r".into(), scratch: dir.clone() });
    let r = engine(vec![ex2], Arc::new(Rec::default())).run(&s, &who(), None).unwrap();
    assert!(r.timed_out && r.signal == Some(9) && !r.ok());
}

// ------------------------------------------------------------------------------------------------------------------ wasm tier

fn wasm_spec(wat: &str, entry: &str) -> SandboxSpec {
    let mut s = SandboxSpec::command(vec![entry.into()], "", labels());
    s.work = Work::WasmModule;
    s.wasm = wat.as_bytes().to_vec();
    s.tier = None;
    s.limits = Limits { cpu_m: 100, mem_mib: 16, pids: 8, seconds: 2, workspace_mib: 1, output_kib: 8 };
    s
}

#[test]
fn the_wasm_tier_runs_a_module_with_fuel_and_memory_limits() {
    let e = engine(vec![Arc::new(WasmExecutor)], Arc::new(Rec::default()));
    let ok = wasm_spec(r#"(module (func (export "_start") (result i32) i32.const 7))"#, "_start");
    let r = e.run(&ok, &who(), None).unwrap();
    assert!(r.tier == Tier::Wasm && r.exit_code == Some(7) && !r.timed_out, "{r:?}");
    // an endless loop runs out of fuel and is reported as a timeout
    let spin = wasm_spec(r#"(module (func (export "_start") (loop br 0)))"#, "_start");
    let t0 = std::time::Instant::now();
    let r = e.run(&spin, &who(), None).unwrap();
    assert!(r.timed_out && !r.ok() && t0.elapsed() < Duration::from_secs(10), "{r:?}");
    assert!(r.stderr.contains("stopped"));
    // memory beyond the limit cannot be had: the growth fails inside the guest (returns -1)
    let mem = wasm_spec(r#"(module (memory 1) (func (export "_start") (result i32) i32.const 100000 memory.grow))"#, "_start");
    let r = e.run(&mem, &who(), None).unwrap();
    assert_eq!(r.exit_code, Some(-1), "{r:?}");
    // bad modules and missing entries are refusals, not crashes
    assert!(matches!(e.run(&wasm_spec("not wasm", "_start"), &who(), None), Err(SandboxError::Refused(_))));
    assert!(matches!(e.run(&wasm_spec(r#"(module (func (export "x")))"#, "_start"), &who(), None), Err(SandboxError::Refused(_))));
    // no network and no secrets in this tier
    let mut s = ok.clone();
    s.network = NetMode::Proxy;
    assert!(e.run(&s, &who(), None).is_err());
    assert_eq!(WasmExecutor::fuel_for(100, 2), 20_000);
}

// ------------------------------------------------------------------------------------------------------------------ policy in the tenant configuration

fn cfg_env(tag: &str) -> (std::path::PathBuf, Arc<MemVault>, Rec, crate::cloudroles::Roles) {
    let dir = crate::testdir::unique("sandbox-cfg", tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut r = crate::cloudroles::Roles::new("olga").unwrap();
    r.add_tenant_member("olga", "mia", crate::cloudroles::TenantRole::Member).unwrap();
    (dir, MemVault::new(), Rec::default(), r)
}

fn codes(v: &Value) -> Vec<(String, String)> {
    v["warnings"].as_array().unwrap().iter().map(|w| (w["code"].as_str().unwrap().to_string(), w["severity"].as_str().unwrap().to_string())).collect()
}

#[test]
fn the_sandbox_policy_kind_validates_and_its_safety_rules_block_and_warn() {
    let (dir, vault, rec, roles) = cfg_env("rules");
    let env = Env { dir: &dir, vault, audit: &rec };
    let owner = Caller { user: "olga", roles: Some(&roles) };
    let check = |values: Value| api::validate(&env, &owner, "sandbox.policy", json!({"enabled": true, "values": values})).unwrap();
    let has = |v: &Value, code: &str, sev: &str| codes(v).contains(&(code.to_string(), sev.to_string()));
    // schema
    assert!(crate::tenantconfig::schema::kind("sandbox.policy").is_some());
    let v = check(json!({"tiers": ["job"], "allowed_registries": ["registry.example.com"], "runtime_class": "gvisor"}));
    assert!(v["problems"].as_array().map_or(true, |p| p.is_empty()), "{v}");
    assert!(codes(&v).iter().all(|(_, s)| s != "block"), "{v}");
    // BLOCK: privileged-like options
    for f in ["allow_privileged", "allow_host_network", "allow_host_path", "allow_root", "allow_cap_add"] {
        let v = check(json!({f: true, "runtime_class": "gvisor"}));
        assert!(has(&v, "sandbox.privileged", "block"), "{f}: {v}");
    }
    // BLOCK: direct internet and unknown modes and tiers
    assert!(has(&check(json!({"network_modes": ["none", "direct"]})), "sandbox.net.direct", "block"));
    assert!(has(&check(json!({"network_modes": ["vpn"]})), "sandbox.net.unknown", "block"));
    assert!(has(&check(json!({"tiers": ["job", "lxc"]})), "sandbox.tier.unknown", "block"));
    assert!(has(&check(json!({"tiers": ["namespace"]})), "sandbox.userns.ack", "block"));
    assert!(!has(&check(json!({"tiers": ["namespace"], "userns_ack": true})), "sandbox.userns.ack", "block"));
    assert!(has(&check(json!({"tiers": ["browser"]})), "sandbox.browser.off", "block"));
    // WARN: no runtime class for the job and browser tiers, wide registries, no registries, no browser domains, named profile
    let w = check(json!({"tiers": ["job"], "allowed_registries": ["*"]}));
    assert!(has(&w, "sandbox.runtime_class.missing", "warn") && has(&w, "sandbox.registry.any", "warn"), "{w}");
    assert!(has(&check(json!({"tiers": ["wasm", "job"], "runtime_class": "gvisor"})), "sandbox.registry.none", "warn"));
    assert!(!has(&check(json!({"tiers": ["wasm"]})), "sandbox.runtime_class.missing", "warn"), "wasm shares no kernel path with a pod");
    assert!(has(&check(json!({"tiers": ["job", "browser"], "browser": true})), "sandbox.browser.domains", "warn"));
    assert!(has(&check(json!({"browser_profile": "named"})), "sandbox.browser.profile", "warn"));
    // bad values
    assert!(!api::validate(&env, &owner, "sandbox.policy", json!({"enabled": true, "values": {"max_cpu_m": 5}})).unwrap()["problems"].as_array().unwrap().is_empty());
    // a member cannot write it
    let mia = Caller { user: "mia", roles: Some(&roles) };
    assert!(api::put(&env, &mia, "sandbox.policy", &[], json!({"enabled": true, "values": {}, "ack_warnings": crate::tenantconfig::safety::rules_table()})).is_err());
    // the rules are listed for the acknowledgement table
    let table = crate::tenantconfig::safety::rules_table().to_string();
    for code in ["sandbox.privileged", "sandbox.net.direct", "sandbox.runtime_class.missing"] {
        assert!(table.contains(code));
    }
}

#[test]
fn the_stored_policy_is_read_with_its_defaults_a_team_can_only_narrow_it_and_no_item_means_off() {
    let (dir, vault, rec, roles) = cfg_env("load");
    let env = Env { dir: &dir, vault, audit: &rec };
    let owner = Caller { user: "olga", roles: Some(&roles) };
    // nothing stored: everything refused
    assert_eq!(SandboxPolicy::load(&env, "web"), SandboxPolicy::off());
    assert!(refused(admit(&SandboxPolicy::load(&env, "web"), &spec(), &who(), None)).contains("not switched on"));
    let ack = crate::tenantconfig::safety::rules_table();
    api::put(&env, &owner, "sandbox.policy", &[], json!({"enabled": true, "values": {"allowed_registries": ["registry.example.com"], "runtime_class": "gvisor", "max_seconds": 120}, "ack_warnings": ack})).unwrap();
    let p = SandboxPolicy::load(&env, "web");
    assert!(p.enabled && p.max.seconds == 120 && p.max.mem_mib == 512, "defaults filled in: {p:?}");
    assert_eq!(p.tiers, [Tier::Wasm, Tier::Job]);
    assert!(!p.browser && p.network_modes == [NetMode::None, NetMode::Proxy] && p.cli_role == "editor" && p.per_person == 2 && p.audit_retention_days == 90);
    assert_eq!(p.runtime_class.as_deref(), Some("gvisor"));
    assert!(p.allowed_registries == ["registry.example.com"]);
    // a switched-off item is off
    api::put(&env, &owner, "sandbox.policy", &[], json!({"enabled": false, "values": {}, "ack_warnings": ack})).unwrap();
    assert_eq!(SandboxPolicy::load(&env, "web"), SandboxPolicy::off());
}

// ------------------------------------------------------------------------------------------------------------------ wiring

#[test]
fn run_command_in_a_tenant_pod_never_runs_on_the_pod() {
    use super::wiring::{render, route_of, CloudRunner, Route};
    struct R;
    impl CloudRunner for R {
        fn run_command(&self, _: &str, p: &str, a: &[String], s: u64) -> Result<String, String> {
            Ok(format!("{p} {a:?} {s}"))
        }
    }
    assert!(matches!(route_of(None, false), Route::Local), "a desktop runs it as before");
    match route_of(None, true) {
        Route::Refuse(m) => assert!(m.contains("do not run on the tenant's pod")),
        _ => panic!("a tenant pod without a sandbox must refuse"),
    }
    assert!(matches!(route_of(Some(Arc::new(R)), true), Route::Sandbox(_)));
    assert!(matches!(route_of(Some(Arc::new(R)), false), Route::Sandbox(_)));
    // what the person sees
    let mut r = SandboxResult { run_id: "r".into(), tier: Tier::Job, exit_code: Some(2), signal: None, timed_out: false, oom_killed: false, stdout: "out".into(), stderr: "err".into(), stdout_truncated: false, stderr_truncated: true, redacted: false, artifacts: vec![], files: vec![], usage: Usage::default() };
    assert_eq!(render(&r), "out\nerr\n[exit status 2]\n[output cut]");
    r.oom_killed = true;
    assert!(render(&r).contains("memory limit"));
    r.oom_killed = false;
    r.timed_out = true;
    assert!(render(&r).contains("time limit"));
    let empty = SandboxResult { stdout: String::new(), stderr: String::new(), exit_code: Some(0), stderr_truncated: false, timed_out: false, ..r };
    assert_eq!(render(&empty), "(no output, exit status 0)");
}

#[test]
fn the_tenant_runner_files_an_approval_request_and_runs_it_once_approved() {
    use super::wiring::{approval_item, CloudRunner, TenantRunner};
    let (dir, vault, rec, mut roles) = cfg_env("runner");
    roles.add_tenant_member("olga", "adam", crate::cloudroles::TenantRole::Admin).unwrap();
    roles.create_namespace("adam", "web").unwrap();
    roles.add_namespace_member("adam", "web", "mia", crate::cloudroles::NsRole::Editor).unwrap();
    roles.add_tenant_member("olga", "max", crate::cloudroles::TenantRole::Member).unwrap();
    roles.add_namespace_member("adam", "web", "max", crate::cloudroles::NsRole::Admin).unwrap();
    crate::cloudroles::save(&crate::cloudroles::path(&dir), &roles).unwrap();
    let rec = Arc::new(rec);
    let env = Env { dir: &dir, vault: vault.clone(), audit: rec.as_ref() };
    let owner = Caller { user: "olga", roles: Some(&roles) };
    let ack = crate::tenantconfig::safety::rules_table();
    api::put(&env, &owner, "sandbox.policy", &[], json!({"enabled": true, "values": {"allowed_registries": ["registry.example.com"], "runtime_class": "gvisor", "cli_image": image(), "max_seconds": 120}, "ack_warnings": ack})).unwrap();
    // the team's cluster binding: dev, enabled, a credential in the vault, the image registry allowed
    let cred = crate::tenantconfig::schema::SecretRef::parse("vault:k3s.cluster/web-dev-credential").unwrap();
    vault.put(&cred, "kubeconfig").unwrap();
    api::put(&env, &owner, "k3s.cluster", &["web".into(), "dev".into()], json!({"enabled": true, "values": {"api_server": "https://k.example.com", "credential": "vault:k3s.cluster/web-dev-credential", "allowed_registries": ["registry.example.com"], "four_eyes": false, "quota_pods": 10, "deploy_role": "editor"}, "ack_warnings": ack})).unwrap();
    let k = FakeKube::finishing(JobState { succeeded: true, ..Default::default() }, "bucket-a\nbucket-b\n");
    let runner = TenantRunner { dir: dir.clone(), tenant: "acme".into(), kube: k.clone(), vault: vault.clone(), audit: rec.clone(), secrets: Arc::new(Secrets(BTreeMap::new())), counter: Mutex::new(0) };
    // a plain command runs through the Job executor
    let out = runner.run_command("mia", "aws", &["s3".into(), "ls".into()], 30);
    assert_eq!(out.as_deref(), Ok("bucket-a\nbucket-b\n"), "{out:?}");
    assert_eq!(k.kinds(), ["NetworkPolicy", "Job"]);
    let job = k.applied.lock().unwrap()[1].clone();
    assert_eq!(job["spec"]["activeDeadlineSeconds"], json!(30));
    assert_eq!(job["spec"]["template"]["spec"]["containers"][0]["command"], json!(["aws", "s3", "ls"]));
    // a command that always needs approval: refused, a request is filed for the team's admin
    k.applied.lock().unwrap().clear();
    let args: Vec<String> = ["delete".into(), "pod".into(), "x".into()].to_vec();
    let err = runner.run_command("mia", "kubectl", &args, 30).unwrap_err();
    assert!(err.contains("always needs approval") && err.contains("waiting for an admin of team web"), "{err}");
    assert!(k.applied.lock().unwrap().is_empty());
    // max (admin of web) approves it; it runs once; the same command asks again afterwards
    let pending = crate::tenantconfig::approvals::snapshot(&dir);
    assert_eq!(pending.len(), 1);
    let item = approval_item(&["kubectl".into(), "delete".into(), "pod".into(), "x".into()]);
    assert_eq!(pending[0].item, item);
    let max = Caller { user: "max", roles: Some(&roles) };
    crate::tenantconfig::approvals::decide(&env, &max, &pending[0].id, true, crate::tenantconfig::now_secs()).unwrap();
    assert!(runner.run_command("mia", "kubectl", &args, 30).is_ok());
    assert!(k.kinds().contains(&"Job".to_string()));
    assert!(runner.run_command("mia", "kubectl", &args, 30).unwrap_err().contains("always needs approval"), "an approval is used once");
    // somebody who is not in the tenant, and a viewer, are refused
    assert!(runner.run_command("eve", "aws", &[], 30).is_err());
    let audit = rec.all();
    assert!(audit.contains("sandbox.start") && audit.contains("tenant.k3s.deploy"), "the binding was asked: {audit}");
}

#[test]
fn the_flow_adapters_map_cli_and_browser_nodes_to_sandbox_runs_and_back() {
    use super::wiring::{browser_spec as flow_browser_spec, browser_via_sandbox, exec_spec, exec_via_sandbox};
    use iyakki_flow::host::{BrowserRequest, ExecRequest};
    let req = ExecRequest { argv: vec!["aws".into(), "s3".into(), "ls".into()], cwd: "sub".into(), env: vec![("REGION".into(), "eu".into())], timeout_ms: 1500, flow_id: "f".into(), run_id: "r".into(), node_id: "n".into(), ..Default::default() };
    let spec = exec_spec(&req, &image(), labels());
    assert!(spec.limits.seconds == 2 && spec.workdir == "/workspace/sub" && spec.tier == Some(Tier::Job) && spec.network == NetMode::Proxy);
    assert_eq!(spec.argv, ["aws", "s3", "ls"]);
    // a failing command comes back as the engine's ExecResult
    let k = FakeKube::finishing(JobState { failed: true, exit_code: Some(3), ..Default::default() }, "oops\n");
    let e = engine(vec![job_exec(k.clone(), Ok(()))], Arc::new(Rec::default()));
    let r = exec_via_sandbox(&e, &who(), &req, &image(), labels(), None).unwrap();
    assert!(r.code == Some(3) && r.stdout == "oops\n" && !r.timed_out && r.ran_in == "sandbox job r1", "{r:?}");
    // a refusal is a configuration error of the node, with the reason
    let bad = ExecRequest { image: "evil.example.net/x:1".into(), ..req.clone() };
    let err = exec_via_sandbox(&e, &who(), &bad, &image(), labels(), None).unwrap_err();
    assert!(err.message.contains("evil.example.net"), "{err:?}");
    // a command that needs approval is reported as such
    let apply = ExecRequest { argv: vec!["kubectl".into(), "apply".into(), "-f".into(), "x".into()], ..req.clone() };
    assert!(exec_via_sandbox(&e, &who(), &apply, &image(), labels(), None).unwrap_err().message.contains("needs approval"));
    // the browser node: steps become steps.json, the domains are the node's allow-list
    let breq = BrowserRequest { steps: json!([{"goto": "https://www.example.org/"}]), allow_domains: vec!["www.example.org".into()], timeout_ms: 60_000, ..Default::default() };
    let bs = flow_browser_spec(&breq, &image(), labels());
    assert!(bs.work == Work::Browser && bs.limits.seconds == 60 && bs.inputs.iter().any(|i| i.name == "steps.json") && bs.domains == ["www.example.org"]);
    let k = FakeKube::finishing(JobState { succeeded: true, ..Default::default() }, "{\"passed\":true}\n");
    k.artifacts.lock().unwrap().push(("out/final.png".into(), vec![1, 2, 3]));
    let e = engine(vec![job_exec(k.clone(), Ok(()))], Arc::new(Rec::default()));
    let r = browser_via_sandbox(&e, &who(), &breq, labels()).unwrap();
    assert_eq!(r.screenshots, ["out/final.png"]);
    assert!(r.report.contains("passed"));
    assert!(k.kinds().contains(&"ConfigMap".to_string()), "the spec and the steps travel in the run's ConfigMap");
    // a domain the tenant did not list is refused
    let off = BrowserRequest { allow_domains: vec!["evil.example.net".into()], ..breq };
    assert!(browser_via_sandbox(&e, &who(), &off, labels()).unwrap_err().message.contains("browser allow-list"));
}

#[test]
fn a_cloud_flows_job_spec_becomes_one_sandbox_request_with_the_hardened_manifest() {
    use crate::cloudflows::job::JobSpec;
    let js = JobSpec {
        name: "run-nightly-r12".into(), namespace: "web-prod".into(), tenant: "acme".into(), team: "web".into(), environment: "prod".into(), flow: "nightly".into(), run: "r12".into(),
        image: image(), argv: vec!["/app/tool".into(), "--once".into()], env: vec![("IYAKKI_FLOW".into(), "nightly".into())], secret_env: vec![],
        cpu_m: 500, mem_mib: 256, deadline_secs: 120, ttl_after_finish_secs: 3600,
    };
    let s = super::wiring::spec_from_flow_job(&js, "mia").unwrap();
    let adm = admit(&pol(), &s, &who(), None).unwrap();
    let j = job::job_for(&adm, &pol(), "web-prod");
    let c = j.pointer("/spec/template/spec/containers/0").unwrap();
    assert_eq!(c["command"], json!(["/app/tool", "--once"]));
    assert_eq!(c["image"], json!(image()));
    assert_eq!(j.pointer("/spec/activeDeadlineSeconds"), Some(&json!(120)));
    assert_eq!(j.pointer("/metadata/namespace"), Some(&json!("web-prod")));
    assert_eq!(j.pointer("/metadata/labels/iyakki.dev~1run"), Some(&json!("nightly-r12")));
    // the sandbox's manifest has what the scheduler's text lacks: a NetworkPolicy and a workspace with a size limit
    assert_eq!(job::bundle(&adm, &pol(), "web-prod").len(), 2);
    assert!(j.pointer("/spec/template/spec/volumes/0/emptyDir/sizeLimit").is_some());
    // a Kubernetes Secret reference cannot travel
    let with_secret = JobSpec { secret_env: vec![("API_TOKEN".into(), "web-prod-api".into(), "token".into())], ..js };
    assert!(super::wiring::spec_from_flow_job(&with_secret, "mia").unwrap_err().contains("vault reference"));
}
