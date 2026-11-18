//! The flow-facing description of one run that becomes a Job. There is ONE Job builder in the program, the sandbox layer's
//! (`sandbox::job::bundle`: Job, per-run NetworkPolicy, workspace volume with a size limit, runtime class, non-root, read-only root,
//! no privilege escalation, capabilities dropped, seccomp, no service-account token, deadline, time to live, resource limits).
//! [`JobSpec`] is what the scheduler knows about a run; `sandbox::wiring::spec_from_flow_job` turns it into a sandbox request and
//! [`bundle_text`] runs admission and builds the documents. Secrets are never in a spec: a Kubernetes Secret reference (`secret_env`)
//! is refused by the sandbox, flows use `vault:` references.

use crate::sandbox::admit::{admit, Who};
use crate::sandbox::policy::SandboxPolicy;
use serde_json::json;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSpec {
    /// DNS label: the Job's name (the sandbox derives its own from the run).
    pub name: String,
    pub namespace: String,
    pub tenant: String,
    pub team: String,
    pub environment: String,
    pub flow: String,
    pub run: String,
    pub image: String,
    /// The program and its arguments; run directly, never through a shell.
    pub argv: Vec<String>,
    /// Plain settings (names and values).
    pub env: Vec<(String, String)>,
    /// Kubernetes Secret references: refused (see above); kept so the refusal can be explained.
    pub secret_env: Vec<(String, String, String)>,
    pub cpu_m: u32,
    pub mem_mib: u32,
    pub deadline_secs: u64,
    pub ttl_after_finish_secs: u64,
}

/// `<registry>` of an image reference (`registry.example/app:1` gives `registry.example`; no slash gives `docker.io`).
pub fn registry_of(image: &str) -> String {
    match image.split_once('/') {
        Some((h, _)) if h.contains('.') || h.contains(':') || h == "localhost" => h.to_string(),
        _ => "docker.io".into(),
    }
}

/// Admission by the tenant's sandbox policy, then the documents of the run as one `List` (JSON, which `kubectl apply -f` takes).
pub fn bundle_text(spec: &JobSpec, person: &str, role: &str, policy: &SandboxPolicy, namespace: &str) -> Result<String, String> {
    let sb = crate::sandbox::wiring::spec_from_flow_job(spec, person)?;
    let adm = admit(policy, &sb, &Who { person, role }, None).map_err(|e| e.to_string())?;
    let items = crate::sandbox::job::bundle(&adm, policy, namespace);
    serde_json::to_string_pretty(&json!({"apiVersion": "v1", "kind": "List", "items": items})).map_err(|e| e.to_string())
}
