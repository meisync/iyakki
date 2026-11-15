//! The fallback runtime of Linux apps as a pod in the dev cluster (F-LINAPP-14): the archived root of a sandbox becomes an image the
//! cluster can import, and a locked-down pod of that image is started. For a kernel without namespaces (the IYAKKI kernels have them, so
//! this is the way out for other machines and for a sandbox the person wants to keep running as a pod).
//!
//! 1. the archive (a `.tar.gz` or a `.tar` of the root, made inside the sandbox) is unpacked in memory and rewritten into ONE clean image
//!    layer by [`crate::ocipush_root`] (F-LINAPP-16: devices, set-id bits, owners and times gone, unsafe names refused);
//! 2. the image is saved as an OCI layout and imported into the cluster's containerd (`k3s ctr images import`, through [`Importer`]);
//! 3. the pod of [`crate::linuxapps::pod_manifest`] (no token, no privilege, all capabilities dropped, `imagePullPolicy: Never` because the
//!    image only exists on the node) is applied through [`Kube`] and waited for until it runs;
//! 4. the answer names the pod and the exact `kubectl exec` command ([`crate::linuxapps::pod_exec_argv`]).
//!
//! Nothing here talks to a cluster by itself: the tests give a fake [`Kube`] and a fake [`Importer`]. [`stop`] deletes the pod; the
//! imported image stays on the node until the person removes it.

use std::path::Path;

use serde_json::json;

use crate::k8s::Kube;
use crate::linuxapps::{pod_exec_argv, pod_manifest};
use crate::ocipush::layout_tar;
use crate::ocipush_root::{build_image_from_root, RootImageSpec, RootReport};

pub const NAMESPACE: &str = "iyakki-apps";
/// The most the compressed archive may be.
pub const MAX_ARCHIVE: usize = 256 << 20;
/// The most the unpacked root may be (the image is built in memory).
pub const MAX_ROOT: u64 = 768 << 20;

/// Imports an OCI layout file into the cluster's image store (`k3s ctr images import FILE`).
pub trait Importer: Send + Sync {
    fn import(&self, file: &Path) -> Result<(), String>;
}

#[derive(Clone, Debug)]
pub struct PodSpec {
    /// The person (for the pod's name).
    pub user: String,
    /// The sandbox's name: lower-case letters, digits and `-`.
    pub name: String,
    /// `amd64` or `arm64`.
    pub arch: String,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub workdir: String,
    pub env: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct Started {
    pub pod: String,
    pub image: String,
    pub report: RootReport,
    /// The command that runs a program in the pod.
    pub exec: String,
}

pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 30 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !s.starts_with('-') && !s.ends_with('-')
}

/// The pod's name for a person and a sandbox.
pub fn pod_name(user: &str, name: &str) -> String {
    pod_manifest(user, name, "x")["metadata"]["name"].as_str().unwrap_or("linuxapp").to_string()
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// The root as a plain tar: a gzip stream is unpacked (capped), a tar is taken as it is.
pub fn unpack_archive(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.is_empty() || bytes.len() > MAX_ARCHIVE {
        return Err(format!("the archive is empty or larger than {} MB", MAX_ARCHIVE >> 20));
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        iyakki_proxy::https::gunzip(bytes, MAX_ROOT as usize + (8 << 20)).map_err(|e| format!("the archive is not a readable .tar.gz ({e})"))
    } else if bytes.len() > 512 && bytes[257..262] == *b"ustar" {
        Ok(bytes.to_vec())
    } else {
        Err("the archive is neither a .tar.gz nor a .tar (make it inside the sandbox with `tar -C / -czf root.tar.gz ...`)".into())
    }
}

/// Convert the archived root of a sandbox into an image, import it and start the pod. `work` is a private folder for the layout file (it is
/// removed again); `sleep` is how the caller waits between two looks at the pod (a test passes a no-op).
pub fn start(kube: &dyn Kube, imp: &dyn Importer, archive: &[u8], spec: &PodSpec, work: &Path, max_looks: u32, sleep: &dyn Fn()) -> Result<Started, String> {
    if !valid_name(&spec.name) {
        return Err("the sandbox name is lower-case letters, digits and - (up to 30)".into());
    }
    let tar = unpack_archive(archive)?;
    let rspec = RootImageSpec { arch: &spec.arch, user: "10001:10001", entrypoint: spec.entrypoint.clone(), cmd: spec.cmd.clone(), env: spec.env.clone(), workdir: &spec.workdir, port: None };
    let (img, report) = build_image_from_root(&tar, &rspec, MAX_ROOT)?;
    let digest = img.layer_digest.trim_start_matches("sha256:").chars().take(12).collect::<String>();
    let reference = format!("iyakki.local/linuxapp-{}-{}:{digest}", spec.user.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).take(20).collect::<String>(), spec.name);
    let layout = layout_tar(&img, &reference)?;
    // the layout is written for the importer and removed whatever happens
    std::fs::create_dir_all(work).map_err(|e| format!("cannot make the work folder: {e}"))?;
    let file = work.join(format!("{}.oci.tar", spec.name));
    iyakki_schema::write_atomic(&file, &layout, 0o600)?;
    let imported = imp.import(&file);
    let _ = std::fs::remove_file(&file);
    imported.map_err(|e| format!("the cluster could not import the image: {e}"))?;
    // the pod: the image only exists on the node, so it is never pulled
    let mut pod = pod_manifest(&spec.user, &spec.name, &reference);
    pod["spec"]["containers"][0]["imagePullPolicy"] = json!("Never");
    pod["spec"]["containers"][0]["securityContext"]["runAsNonRoot"] = json!(true);
    let name = pod["metadata"]["name"].as_str().unwrap_or("").to_string();
    let ns = json!({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": NAMESPACE}});
    let text = format!("{}\n---\n{}\n", serde_json::to_string(&ns).map_err(|e| e.to_string())?, serde_json::to_string(&pod).map_err(|e| e.to_string())?);
    let apply = work.join(format!("{}.pod.json", spec.name));
    iyakki_schema::write_atomic(&apply, text.as_bytes(), 0o600)?;
    let applied = kube.run(&args(&["apply", "-f", &apply.display().to_string()]));
    let _ = std::fs::remove_file(&apply);
    if !applied.ok() {
        return Err(format!("the cluster refused the pod: {}", applied.combined().lines().last().unwrap_or("no answer").trim()));
    }
    let mut phase = String::new();
    for _ in 0..max_looks.max(1) {
        phase = kube.run(&args(&["-n", NAMESPACE, "get", "pod", &name, "-o", "jsonpath={.status.phase}"])).stdout.trim().to_string();
        if phase == "Running" || phase == "Failed" {
            break;
        }
        sleep();
    }
    if phase != "Running" {
        let why = kube.run(&args(&["-n", NAMESPACE, "get", "pod", &name, "-o", "jsonpath={.status.containerStatuses[0].state.waiting.reason}"])).stdout.trim().to_string();
        kube.run(&args(&["-n", NAMESPACE, "delete", "pod", &name, "--ignore-not-found", "--wait=false"]));
        return Err(format!("the pod did not start (it is {}{}); it was removed", if phase.is_empty() { "not there" } else { &phase }, if why.is_empty() { String::new() } else { format!(": {why}") }));
    }
    let exec = pod_exec_argv(&name, &["/bin/sh".to_string()]).join(" ");
    Ok(Started { pod: name, image: reference, report, exec })
}

/// Remove the pod of a sandbox (the imported image stays on the node).
pub fn stop(kube: &dyn Kube, user: &str, name: &str) -> Result<String, String> {
    if !valid_name(name) {
        return Err("the sandbox name is lower-case letters, digits and - (up to 30)".into());
    }
    let pod = pod_name(user, name);
    let got = kube.run(&args(&["-n", NAMESPACE, "get", "pod", &pod, "-o", "name"]));
    if !got.ok() {
        return Err(format!("there is no pod {pod} in the dev cluster"));
    }
    let o = kube.run(&args(&["-n", NAMESPACE, "delete", "pod", &pod, "--wait=false"]));
    if o.ok() { Ok(pod) } else { Err(format!("the cluster did not remove the pod: {}", o.combined().lines().last().unwrap_or("no answer").trim())) }
}

pub fn pods_of(kube: &dyn Kube, user: &str) -> Vec<String> {
    let prefix = pod_name(user, "x").trim_end_matches('x').to_string();
    let o = kube.run(&args(&["-n", NAMESPACE, "get", "pods", "-o", "name"]));
    o.stdout.lines().filter_map(|l| l.trim().strip_prefix("pod/")).filter(|p| p.starts_with(&prefix)).map(str::to_string).collect::<Vec<_>>()
}

