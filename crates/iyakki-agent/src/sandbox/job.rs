//! Pure manifest builders. They take an [`Admitted`] (so a refused request has no manifest) and return JSON values that `kubectl apply`
//! accepts. Every hardening field is written out, never left to a cluster default. Golden tests in `tests.rs` pin them.
//!
//! One run is: a NetworkPolicy (applied first, so the pod never starts unprotected), an optional ConfigMap of input files, an optional
//! Secret of file-delivered secrets, and the Job.
//!
//! NOTE for the cloud-flows work: this is the one place that knows how a sandbox Job looks. [`job_for`] is the entry point; a flow
//! scheduler that needs a Job should build a [`SandboxSpec`] and call it rather than write a second manifest builder.

use super::admit::Admitted;
use super::policy::SandboxPolicy;
use super::spec::{label_map, label_value, Delivery, NetMode, Tier, Work};
use serde_json::{json, Map, Value};

/// A fixed high user id, the same for every run: nothing in the image can be a file owner of another tenant's data.
pub const RUN_UID: u64 = 10001;
pub const TTL_AFTER_FINISHED_SECS: u64 = 600;
/// Memory kept for the pod's `/tmp` (disk, not memory).
pub const TMP_MIB: u32 = 64;
pub const RUN_LABEL: &str = "iyakki.dev/sandbox-run";
pub const SECRETS_DIR: &str = "/run/secrets/iyakki";

fn labels(adm: &Admitted) -> Map<String, Value> {
    label_map(&adm.spec().labels, adm.tier()).into_iter().map(|(k, v)| (k, json!(v))).collect()
}

fn pod_labels(adm: &Admitted) -> Value {
    let mut m = labels(adm);
    m.insert(RUN_LABEL.into(), json!(adm.run_name()));
    Value::Object(m)
}

fn metadata(adm: &Admitted, ns: &str, name: &str) -> Value {
    json!({"name": name, "namespace": ns, "labels": Value::Object(labels(adm))})
}

pub fn proxy_url(policy: &SandboxPolicy, ns: &str) -> String {
    format!("http://{}.{}.svc:{}", policy.proxy.service, ns, policy.proxy.port)
}

/// The per-run NetworkPolicy. `none`: no egress at all. `proxy`: DNS and the egress proxy's pods, nothing else (so not the cluster API,
/// not the node, not the metadata address, not other pods). No ingress in either case.
pub fn network_policy(adm: &Admitted, policy: &SandboxPolicy, ns: &str) -> Value {
    let egress: Vec<Value> = match adm.spec().network {
        NetMode::Proxy => vec![
            json!({
                "to": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}}, "podSelector": {"matchLabels": {"k8s-app": "kube-dns"}}}],
                "ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}]
            }),
            json!({
                "to": [{"podSelector": {"matchLabels": {"app.kubernetes.io/name": policy.proxy.app_label}}}],
                "ports": [{"protocol": "TCP", "port": policy.proxy.port}]
            }),
        ],
        _ => vec![],
    };
    json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": metadata(adm, ns, &adm.run_name()),
        "spec": {
            "podSelector": {"matchLabels": {RUN_LABEL: adm.run_name()}},
            "policyTypes": ["Ingress", "Egress"],
            "ingress": [],
            "egress": egress
        }
    })
}

/// The ConfigMap with the input files, if there are any (the browser tier also keeps its spec there).
pub fn inputs_configmap(adm: &Admitted, ns: &str, policy: &SandboxPolicy) -> Option<Value> {
    let mut data = Map::new();
    for f in &adm.spec().inputs {
        data.insert(f.name.clone(), json!(String::from_utf8_lossy(&f.bytes)));
    }
    if adm.tier() == Tier::Browser {
        data.insert("spec.json".into(), json!(browser_spec(adm, policy).to_string()));
    }
    if data.is_empty() {
        return None;
    }
    Some(json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": metadata(adm, ns, &format!("{}-in", adm.run_name())), "immutable": true, "data": Value::Object(data)}))
}

pub fn has_file_secrets(adm: &Admitted) -> bool {
    adm.spec().secrets.iter().any(|s| s.delivery == Delivery::TmpfsFile)
}

pub fn secret_name(adm: &Admitted) -> String {
    format!("{}-sec", adm.run_name())
}

/// The Secret object for the file-delivered secrets; the values come from the resolver at apply time and are sent through the
/// API call's standard input, never written to a file or a command line. Deleted when the run ends.
pub fn secret_object(adm: &Admitted, ns: &str, values: &std::collections::BTreeMap<String, String>) -> Value {
    let data: Map<String, Value> = values.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    json!({"apiVersion": "v1", "kind": "Secret", "metadata": metadata(adm, ns, &secret_name(adm)), "type": "Opaque", "immutable": true, "stringData": Value::Object(data)})
}

fn quantity_mi(n: u32) -> String {
    format!("{n}Mi")
}

fn cpu(n: u32) -> String {
    format!("{n}m")
}

/// The `iyakki browser-test` spec of a browser run: the start addresses, the steps the caller gave in `steps.json`, a final screenshot.
/// `allow_domains` is the tenant's list narrowed to the hosts of the start addresses plus the domains the steps need (none implied).
pub fn browser_spec(adm: &Admitted, policy: &SandboxPolicy) -> Value {
    let spec = adm.spec();
    let mut steps: Vec<Value> = spec.argv.iter().map(|u| json!({"goto": u})).collect();
    if let Some(extra) = spec.inputs.iter().find(|i| i.name == "steps.json") {
        if let Ok(Value::Array(a)) = serde_json::from_slice::<Value>(&extra.bytes) {
            steps.extend(a);
        }
    }
    steps.push(json!({"screenshot": "final"}));
    json!({
        "name": format!("sandbox {}", adm.run_name()),
        "allow_domains": spec.browser_domains(),
        "timeout_ms": u64::from(spec.limits.seconds.min(policy.browser_session_secs)) * 1000,
        "allow_payment": false,
        "allow_destructive": false,
        "steps": steps
    })
}

/// The hardened Job of one run (tiers `job` and `browser`).
pub fn job_for(adm: &Admitted, policy: &SandboxPolicy, ns: &str) -> Value {
    let spec = adm.spec();
    let browser = adm.tier() == Tier::Browser;
    let name = adm.run_name();
    let image = if spec.image.is_empty() { policy.browser_image.clone() } else { spec.image.clone() };
    let seconds = if browser { spec.limits.seconds.min(policy.browser_session_secs) } else { spec.limits.seconds };
    let workdir = spec.workdir_or_default().to_string();
    let command: Vec<String> = if browser {
        ["iyakki", "browser-test", "/inputs/spec.json", "--out", "/workspace/out", "--json", "--profile", "/profile"].iter().map(|s| s.to_string()).collect()
    } else {
        spec.argv.clone()
    };

    let mut env: Vec<Value> = spec.env.iter().map(|e| json!({"name": e.name, "value": e.value})).collect();
    env.push(json!({"name": "HOME", "value": "/workspace"}));
    env.push(json!({"name": "TMPDIR", "value": "/tmp"}));
    if spec.network == NetMode::Proxy {
        let p = proxy_url(policy, ns);
        for k in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            env.push(json!({"name": k, "value": p}));
        }
        for k in ["NO_PROXY", "no_proxy"] {
            env.push(json!({"name": k, "value": "localhost,127.0.0.1"}));
        }
    }
    if browser {
        env.push(json!({"name": "IYAKKI_BROWSER_MEDIA", "value": "deny"}));
        env.push(json!({"name": "IYAKKI_WEBVIEW_HELPER", "value": "/usr/libexec/iyakki/webview-helper"}));
    }

    let mut mounts = vec![json!({"name": "workspace", "mountPath": "/workspace"}), json!({"name": "tmp", "mountPath": "/tmp"})];
    let mut volumes = vec![
        json!({"name": "workspace", "emptyDir": {"sizeLimit": quantity_mi(spec.limits.workspace_mib)}}),
        json!({"name": "tmp", "emptyDir": {"sizeLimit": quantity_mi(TMP_MIB)}}),
    ];
    if inputs_configmap(adm, ns, policy).is_some() {
        mounts.push(json!({"name": "inputs", "mountPath": "/inputs", "readOnly": true}));
        volumes.push(json!({"name": "inputs", "configMap": {"name": format!("{name}-in"), "defaultMode": 0o444}}));
    }
    if has_file_secrets(adm) {
        mounts.push(json!({"name": "secrets", "mountPath": SECRETS_DIR, "readOnly": true}));
        volumes.push(json!({"name": "secrets", "secret": {"secretName": secret_name(adm), "defaultMode": 0o400}}));
    }
    let mut ephemeral_mib = spec.limits.workspace_mib + TMP_MIB;
    if browser {
        // the profile lives in memory and is gone with the pod; a named profile is the one thing that may survive, and only when the tenant allows it
        if policy.browser_named_profile {
            volumes.push(json!({"name": "profile", "persistentVolumeClaim": {"claimName": format!("sbx-profile-{}-{}", label_value(&spec.labels.team), label_value(&spec.labels.person))}}));
        } else {
            volumes.push(json!({"name": "profile", "emptyDir": {"medium": "Memory", "sizeLimit": quantity_mi(256)}}));
        }
        mounts.push(json!({"name": "profile", "mountPath": "/profile"}));
        volumes.push(json!({"name": "downloads", "emptyDir": {"sizeLimit": quantity_mi(policy.browser_download_mib)}}));
        mounts.push(json!({"name": "downloads", "mountPath": "/downloads"}));
        ephemeral_mib += policy.browser_download_mib;
    }
    let mem = spec.limits.mem_mib + if browser && !policy.browser_named_profile { 256 } else { 0 };

    let mut pod = json!({
        "restartPolicy": "Never",
        "automountServiceAccountToken": false,
        "enableServiceLinks": false,
        "hostNetwork": false,
        "hostPID": false,
        "hostIPC": false,
        "shareProcessNamespace": false,
        "terminationGracePeriodSeconds": 5,
        "securityContext": {
            "runAsNonRoot": true,
            "runAsUser": RUN_UID,
            "runAsGroup": RUN_UID,
            "fsGroup": RUN_UID,
            "seccompProfile": {"type": "RuntimeDefault"}
        },
        "containers": [{
            "name": "run",
            "image": image,
            "imagePullPolicy": "IfNotPresent",
            "command": command,
            "workingDir": workdir,
            "env": env,
            "securityContext": {
                "privileged": false,
                "allowPrivilegeEscalation": false,
                "readOnlyRootFilesystem": true,
                "runAsNonRoot": true,
                "runAsUser": RUN_UID,
                "runAsGroup": RUN_UID,
                "capabilities": {"drop": ["ALL"]},
                "seccompProfile": {"type": "RuntimeDefault"}
            },
            "resources": {
                "requests": {"cpu": cpu(spec.limits.cpu_m.min(100)), "memory": quantity_mi(mem), "ephemeral-storage": quantity_mi(ephemeral_mib)},
                "limits": {"cpu": cpu(spec.limits.cpu_m), "memory": quantity_mi(mem), "ephemeral-storage": quantity_mi(ephemeral_mib)}
            },
            "volumeMounts": mounts
        }],
        "volumes": volumes
    });
    if let Some(rc) = &policy.runtime_class {
        pod["runtimeClassName"] = json!(rc);
    }
    json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": metadata(adm, ns, &name),
        "spec": {
            "backoffLimit": 0,
            "completions": 1,
            "parallelism": 1,
            "activeDeadlineSeconds": seconds,
            "ttlSecondsAfterFinished": TTL_AFTER_FINISHED_SECS,
            "template": {"metadata": {"labels": pod_labels(adm)}, "spec": pod}
        }
    })
}

/// Everything to apply for a run, in order: NetworkPolicy, inputs, Job. (The Secret of file-delivered secrets is made by the executor
/// from the resolver's values, after the NetworkPolicy and before the Job.)
pub fn bundle(adm: &Admitted, policy: &SandboxPolicy, ns: &str) -> Vec<Value> {
    debug_assert!(matches!(adm.tier(), Tier::Job | Tier::Browser) && adm.spec().work != Work::WasmModule);
    let mut v = vec![network_policy(adm, policy, ns)];
    if let Some(c) = inputs_configmap(adm, ns, policy) {
        v.push(c);
    }
    v.push(job_for(adm, policy, ns));
    v
}
