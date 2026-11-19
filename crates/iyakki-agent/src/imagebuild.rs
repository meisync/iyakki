//! An on-device image builder (F-LINAPP-15; docs/BUILD-YOUR-OWN-IMAGE-APP.md). The IYAKKI image ships no buildah, podman or docker, so a
//! build runs as a JOB IN THE DEV CLUSTER: the workspace folder is packed into a ConfigMap, a builder image runs `buildah bud` and
//! `buildah push` to the in-cluster registry, the job prints the digest, and the result is an image app file (`<data>/image-apps/<id>.json`)
//! pinned by that digest. Nothing here talks to a cluster by itself: [`build`] takes a [`Kube`] runner and the tests give a fake one.
//!
//! * The builder image is NOT chosen here: it must be pinned by digest and given by the caller (the owner picks and tests one, KI-851).
//! * What leaves the folder: files that look like secrets (`.env*`, keys, `id_*`), `.git`, `node_modules` and `target` are left out and
//!   reported; a link is never followed; the whole context is at most [`MAX_CONTEXT`] bytes (a ConfigMap holds under 1 MiB).
//! * The job has no service-account token, a wall-clock limit, resource limits and removes itself; the objects are deleted after the build
//!   whatever happened. Building asks the person first (the chat tool), because it pulls base images from the internet and runs their commands.

use std::path::Path;

use serde_json::{json, Value};

use crate::k8s::Kube;
use crate::ocipush::tar_entry;

pub const NAMESPACE: &str = "iyakki-build";
/// What one build context may weigh (uncompressed tar); the ConfigMap's base64 form stays under the 1 MiB object limit.
pub const MAX_CONTEXT: usize = 700 * 1024;
pub const MAX_FILES: usize = 400;
/// The line the job prints with the image digest.
pub const DIGEST_MARK: &str = "IYAKKI-DIGEST=";

#[derive(Clone, Debug)]
pub struct BuildSpec {
    /// The repository name in the registry (`my-notes`): lower-case letters, digits, `-`, `_`, `.`.
    pub repo: String,
    pub tag: String,
    /// The Containerfile or Dockerfile inside the folder.
    pub dockerfile: String,
    /// The builder image, pinned by digest (`quay.io/buildah/stable@sha256:...`).
    pub builder_image: String,
    /// The in-cluster registry (`localhost:30500`).
    pub registry: String,
}

/// The folder as one tar.
#[derive(Debug)]
pub struct Context {
    pub tar: Vec<u8>,
    pub files: usize,
    pub bytes: usize,
    /// Left out, with the reason in the name's neighbour: `.env (looks like a secret)`.
    pub skipped: Vec<String>,
}

fn skip_reason(rel: &str, name: &str) -> Option<&'static str> {
    let l = name.to_ascii_lowercase();
    if matches!(l.as_str(), ".git" | "node_modules" | "target" | ".iyakki" | "__pycache__") {
        return Some("not part of a build");
    }
    if l == ".env" || l.starts_with(".env.") || l.ends_with(".pem") || l.ends_with(".key") || l.starts_with("id_rsa") || l.starts_with("id_ed25519") || l == ".netrc" || l.ends_with(".p12") || rel.contains("/.ssh/") {
        return Some("looks like a secret");
    }
    None
}

fn walk(base: &Path, rel: &str, out: &mut Vec<(String, std::path::PathBuf)>, skipped: &mut Vec<String>) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(base.join(rel)).map_err(|e| format!("{}: {e}", base.join(rel).display()))?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if let Some(why) = skip_reason(&r, &name) {
            skipped.push(format!("{r} ({why})"));
            continue;
        }
        let t = e.file_type().map_err(|e| e.to_string())?;
        if t.is_dir() {
            walk(base, &r, out, skipped)?;
        } else if t.is_file() {
            out.push((r, e.path()));
        } else {
            skipped.push(format!("{r} (a link or special file)"));
        }
    }
    Ok(())
}

/// Pack the folder. Refused when it is too big, has too many files, holds a name the archive cannot carry, or has no `dockerfile`.
pub fn package_context(dir: &Path, dockerfile: &str) -> Result<Context, String> {
    if dockerfile.is_empty() || dockerfile.starts_with('/') || dockerfile.split('/').any(|p| p == ".." || p.is_empty()) {
        return Err("the Dockerfile name is a path inside the folder".into());
    }
    let (mut files, mut skipped) = (vec![], vec![]);
    walk(dir, "", &mut files, &mut skipped)?;
    if !files.iter().any(|(r, _)| r == dockerfile) {
        return Err(format!("the folder has no {dockerfile}"));
    }
    if files.len() > MAX_FILES {
        return Err(format!("{} files: a build context holds at most {MAX_FILES} (put the rest in .dockerignore-style folders such as node_modules, which are left out)", files.len()));
    }
    let mut tar = Vec::new();
    let mut bytes = 0usize;
    for (rel, p) in &files {
        if rel.len() > 99 {
            return Err(format!("the path {rel} is longer than 99 characters"));
        }
        let data = std::fs::read(p).map_err(|e| format!("{rel}: {e}"))?;
        bytes += data.len();
        if bytes > MAX_CONTEXT {
            return Err(format!("the files add up to more than {} KB: a build context here is at most that (images that need more are built elsewhere)", MAX_CONTEXT / 1024));
        }
        tar.extend_from_slice(&tar_entry(rel, 0o644, &data));
    }
    tar.extend_from_slice(&[0u8; 1024]);
    Ok(Context { tar, files: files.len(), bytes, skipped })
}

fn name_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')) && !s.starts_with(['-', '_', '.'])
}

fn tag_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) && !s.starts_with(['-', '.'])
}

pub fn validate(spec: &BuildSpec) -> Result<(), String> {
    if !name_ok(&spec.repo) {
        return Err("the image name is lower-case letters, digits, - _ and .".into());
    }
    if !tag_ok(&spec.tag) {
        return Err("the tag is letters, digits, - _ and .".into());
    }
    crate::imageapps::validate_image_ref(&spec.builder_image).map_err(|e| format!("the builder image: {e}"))?;
    if !spec.builder_image.contains("@sha256:") || spec.builder_image.contains("@sha256:0000000000000000000000000000000000000000000000000000000000000000") {
        return Err("the builder image is not pinned by a real digest".into());
    }
    let (host, port) = spec.registry.rsplit_once(':').ok_or("the registry is host:port")?;
    if !(host == "localhost" || host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')) || port.parse::<u16>().is_err() {
        return Err("the registry is host:port".into());
    }
    if spec.dockerfile.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '$' || c == '`' || c == ';') {
        return Err("the Dockerfile name has a character a shell would read".into());
    }
    Ok(())
}

/// The ConfigMap with the context and the Job that builds it. `id` is a short unique word of this build (lower-case letters and digits).
pub fn manifests(spec: &BuildSpec, ctx: &Context, id: &str) -> Result<String, String> {
    validate(spec)?;
    if id.is_empty() || id.len() > 24 || !id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
        return Err("a build id is lower-case letters and digits".into());
    }
    let target = format!("{}/{}:{}", spec.registry, spec.repo, spec.tag);
    // every value that reaches the shell passed `validate`; the script itself is fixed text
    let script = format!(
        "set -eu; mkdir -p /work && cd /work && tar -xf /ctx/context.tar && buildah --storage-driver vfs bud --isolation chroot -f '{df}' -t '{target}' . && buildah --storage-driver vfs push --tls-verify=false --digestfile /tmp/digest '{target}' && echo {mark}$(cat /tmp/digest)",
        df = spec.dockerfile,
        mark = DIGEST_MARK
    );
    let b64 = crate::story::base64_encode(&ctx.tar);
    let cm = json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": {"name": format!("build-{id}-ctx"), "namespace": NAMESPACE, "labels": {"app.kubernetes.io/managed-by": "iyakki", "iyakki.dev/build": id}},
        "binaryData": {"context.tar": b64}
    });
    let job = json!({
        "apiVersion": "batch/v1", "kind": "Job",
        "metadata": {"name": format!("build-{id}"), "namespace": NAMESPACE, "labels": {"app.kubernetes.io/managed-by": "iyakki", "iyakki.dev/build": id}},
        "spec": {
            "backoffLimit": 0, "activeDeadlineSeconds": 900, "ttlSecondsAfterFinished": 300,
            "template": {
                "metadata": {"labels": {"iyakki.dev/build": id}},
                "spec": {
                    "restartPolicy": "Never", "automountServiceAccountToken": false,
                    "containers": [{
                        "name": "builder", "image": spec.builder_image, "command": ["sh", "-c", script],
                        "securityContext": {"allowPrivilegeEscalation": false, "capabilities": {"drop": ["ALL"], "add": ["CHOWN", "DAC_OVERRIDE", "FOWNER", "SETUID", "SETGID", "SETFCAP", "KILL"]}},
                        "resources": {"requests": {"cpu": "200m", "memory": "256Mi"}, "limits": {"cpu": "2", "memory": "2Gi", "ephemeral-storage": "4Gi"}},
                        "volumeMounts": [{"name": "ctx", "mountPath": "/ctx", "readOnly": true}]
                    }],
                    "volumes": [{"name": "ctx", "configMap": {"name": format!("build-{id}-ctx")}}]
                }
            }
        }
    });
    let ns = json!({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": NAMESPACE, "labels": {"app.kubernetes.io/managed-by": "iyakki"}}});
    Ok([ns, cm, job].iter().map(|v| serde_json::to_string_pretty(v).unwrap_or_default()).collect::<Vec<_>>().join("\n---\n"))
}

/// The digest the job printed (`IYAKKI-DIGEST=sha256:<64 hex>`).
pub fn digest_from_log(log: &str) -> Option<String> {
    log.lines().filter_map(|l| l.trim().strip_prefix(DIGEST_MARK)).map(str::trim).find(|d| d.strip_prefix("sha256:").is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))).map(str::to_string)
}

/// What a finished build gives back.
#[derive(Debug, PartialEq, Eq)]
pub struct Built {
    /// `localhost:30500/my-notes:1@sha256:...`, ready for the `image` field of an image app.
    pub image: String,
    pub digest: String,
}

/// Run one build: apply, wait, read the log, clean up (always). `sleep` is how the caller waits between two looks (a test passes a no-op);
/// `max_looks` bounds the waiting.
pub fn build(kube: &dyn Kube, spec: &BuildSpec, ctx: &Context, id: &str, apply_file: &Path, max_looks: u32, sleep: &dyn Fn()) -> Result<Built, String> {
    let text = manifests(spec, ctx, id)?;
    iyakki_schema::write_atomic(apply_file, text.as_bytes(), 0o600)?;
    let args = |a: &[&str]| -> Vec<String> { a.iter().map(|s| s.to_string()).collect() };
    let clean = || {
        let _ = std::fs::remove_file(apply_file);
        kube.run(&args(&["-n", NAMESPACE, "delete", "job", &format!("build-{id}"), "--ignore-not-found", "--wait=false"]));
        kube.run(&args(&["-n", NAMESPACE, "delete", "configmap", &format!("build-{id}-ctx"), "--ignore-not-found"]));
    };
    let applied = kube.run(&args(&["apply", "-f", &apply_file.display().to_string()]));
    if !applied.ok() {
        clean();
        return Err(format!("the cluster refused the build: {}", applied.combined().lines().last().unwrap_or("no answer").trim()));
    }
    let job = format!("job/build-{id}");
    let mut state = String::new();
    for _ in 0..max_looks.max(1) {
        let o = kube.run(&args(&["-n", NAMESPACE, "get", &job, "-o", "jsonpath={.status.succeeded},{.status.failed}"]));
        state = o.stdout.trim().to_string();
        if state.starts_with('1') || state.ends_with('1') {
            break;
        }
        sleep();
    }
    let log = kube.run(&args(&["-n", NAMESPACE, "logs", &job, "--tail=200"])).combined();
    clean();
    if state.starts_with('1') {
        return match digest_from_log(&log) {
            Some(d) => Ok(Built { image: format!("{}/{}:{}@{}", spec.registry, spec.repo, spec.tag, d), digest: d }),
            None => Err("the build finished but printed no image digest".into()),
        };
    }
    if state.ends_with('1') {
        let (summary, _, _) = iyakki_lint::dockerlint::build_failure_summary(&log);
        return Err(format!("the build failed: {}", if summary.is_empty() { log.lines().last().unwrap_or("no message").trim().to_string() } else { summary }));
    }
    Err("the build did not finish in time; the objects were removed".into())
}

/// The image app file for a built image (checked by the image apps reader: pinned digest, licence, ports).
pub fn image_app_json(id: &str, title: &str, built: &Built, port: u16, licence: &str) -> Result<String, String> {
    let v: Value = json!({"id": id, "name": title, "description": format!("Built on this computer from your folder ({}).", &built.digest[..19]), "kind": "container", "image": built.image, "command": [], "args": [], "ports": [{"name": "http", "port": port}], "env": [], "volumes": [], "licence": licence, "run_as_user": 10001, "memory_mb": 256, "cpu_milli": 500});
    let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    crate::imageapps::parse(&text)?;
    Ok(text + "\n")
}

#[cfg(test)]
#[path = "imagebuild_tests.rs"]
mod tests;
