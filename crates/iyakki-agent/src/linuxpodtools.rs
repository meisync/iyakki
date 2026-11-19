//! The assistant's tools for the fallback runtime of Linux apps (F-LINAPP-14): `linuxapp_pods` (free: the person's pods), `linuxapp_pod_start`
//! and `linuxapp_pod_stop` (each asks every time, administrators only). Start takes the archived root of a sandbox (a `.tar.gz` of the root,
//! inside the person's own home), turns it into an image the dev cluster can import, imports it and starts a locked-down pod
//! ([`crate::linuxpod`]); the answer is the exact `kubectl exec` command. Nothing starts until the person has read the card.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::k8s::Kube;
use crate::linuxpod::{self as lp, Importer, PodSpec};
pub use iyakki_shared::linuxpodtools::{PROMPT_RULE};

pub const TOOLS: [&str; 3] = ["linuxapp_pods", "linuxapp_pod_start", "linuxapp_pod_stop"];


pub trait Backend: Send + Sync {
    fn is_admin(&self) -> bool;
    fn user(&self) -> String;
    /// The archive `path` names (inside the person's own home), or why not.
    fn archive(&self, path: &str) -> Result<Vec<u8>, String>;
    fn dev_cluster_running(&self) -> bool;
    fn kube(&self) -> Arc<dyn Kube>;
    fn importer(&self) -> Arc<dyn Importer>;
    /// A private folder for the files of one start.
    fn work_dir(&self) -> PathBuf;
    /// `amd64` or `arm64`: what this computer is.
    fn arch(&self) -> String;
    fn audit(&self, action: &str, detail: &str);
}

pub struct LinuxPodTools {
    inner: Arc<dyn Tools>,
    be: Arc<dyn Backend>,
}

impl LinuxPodTools {
    pub fn new(inner: Arc<dyn Tools>, be: Arc<dyn Backend>) -> LinuxPodTools {
        LinuxPodTools { inner, be }
    }

    fn gate(&self) -> Result<(), String> {
        if !self.be.is_admin() {
            return Err("only an administrator account may run a Linux app as a pod".into());
        }
        if !self.be.dev_cluster_running() {
            return Err("the dev cluster is not running: start it in Settings -> Developer first".into());
        }
        Ok(())
    }

    fn spec(&self, a: &Value) -> Result<(Vec<u8>, PodSpec), String> {
        self.gate()?;
        let name = a["name"].as_str().ok_or("give the sandbox name (name)")?.trim().to_string();
        if !lp::valid_name(&name) {
            return Err("the name is lower-case letters, digits and - (up to 30)".into());
        }
        let archive = self.be.archive(a["archive"].as_str().ok_or("give the archive (archive): the .tar.gz of the sandbox root, inside your home")?)?;
        let cmd: Vec<String> = match a.get("command") {
            None | Some(Value::Null) => vec!["/bin/sh".into()],
            Some(Value::Array(v)) if !v.is_empty() && v.len() <= 16 && v.iter().all(|x| x.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 200 && !s.contains('\0'))) => v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
            _ => return Err("command is a short list of words, for example [\"/bin/sh\"]".into()),
        };
        Ok((archive, PodSpec { user: self.be.user(), name, arch: self.be.arch(), entrypoint: vec![], cmd, workdir: "/".into(), env: vec![] }))
    }
}

impl Tools for LinuxPodTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !TOOLS.contains(&d.spec.name.as_str()));
        let obj = |p: Value, r: &[&str]| json!({"type": "object", "properties": p, "required": r});
        v.push(ToolDef { spec: ToolSpec { name: TOOLS[0].into(), description: "List the pods of the person's Linux apps in the dev cluster (names only).".into(), schema: obj(json!({}), &[]) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: TOOLS[1].into(), description: "Turn the archived root of a sandbox (a .tar.gz inside the person's home) into an image, import it into the dev cluster and start a locked-down pod of it. Administrators only. Asks the person every time.".into(), schema: obj(json!({"name": {"type": "string", "description": "the sandbox's name: lower-case letters, digits and -"}, "archive": {"type": "string", "description": "the .tar.gz of the sandbox root, inside the person's home"}, "command": {"type": "array", "items": {"type": "string"}, "description": "the program the image starts by default (default /bin/sh); the pod itself sleeps and is used through exec"}}), &["name", "archive"]) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: TOOLS[2].into(), description: "Remove the pod of a sandbox from the dev cluster (the imported image stays). Asks the person.".into(), schema: obj(json!({"name": {"type": "string"}}), &["name"]) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "linuxapp_pods" => {
                if !self.be.dev_cluster_running() {
                    return Ok("The dev cluster is not running, so there are no pods.".into());
                }
                let pods = lp::pods_of(self.be.kube().as_ref(), &self.be.user());
                Ok(if pods.is_empty() { "No Linux app pods are running.".into() } else { pods.join("\n") })
            }
            "linuxapp_pod_start" => {
                let (archive, spec) = self.spec(args)?;
                let r = lp::start(self.be.kube().as_ref(), self.be.importer().as_ref(), &archive, &spec, &self.be.work_dir(), 60, &|| std::thread::sleep(std::time::Duration::from_secs(2)));
                match r {
                    Ok(s) => {
                        self.be.audit("linuxapp.pod_start", &format!("{} image={} files={}", s.pod, s.image, s.report.files));
                        Ok(format!("The pod {} is running in the dev cluster from {} ({} files, {} folders; {} set-id bits cleared, {} device entries left out). Run a program in it with: {}", s.pod, s.image, s.report.files, s.report.dirs, s.report.setid_cleared, s.report.dropped.len(), s.exec))
                    }
                    Err(e) => {
                        self.be.audit("linuxapp.pod_start", &format!("{} failed", spec.name));
                        Err(e)
                    }
                }
            }
            "linuxapp_pod_stop" => {
                self.gate()?;
                let n = args["name"].as_str().ok_or("give the sandbox name (name)")?;
                let pod = lp::stop(self.be.kube().as_ref(), &self.be.user(), n)?;
                self.be.audit("linuxapp.pod_stop", &pod);
                Ok(format!("The pod {pod} is being removed. The imported image stays in the cluster."))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            "linuxapp_pod_start" => format!("Start the Linux app “{}” as a pod in the dev cluster from {}", args["name"].as_str().unwrap_or("?"), args["archive"].as_str().unwrap_or("an archive")),
            "linuxapp_pod_stop" => format!("Remove the pod of the Linux app “{}”", args["name"].as_str().unwrap_or("?")),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name != TOOLS[1] {
            return self.inner.preview(name, args);
        }
        let (archive, spec) = self.spec(args).ok()?;
        let tar = lp::unpack_archive(&archive).ok()?;
        let (_, rep) = crate::ocipush_root::layer_from_root_tar(&tar, lp::MAX_ROOT).ok()?;
        let mut l = vec![
            format!("Turn the archive ({} KB) into an image: {} files, {} folders, {} links, {} KB unpacked.", archive.len().div_ceil(1024), rep.files, rep.dirs, rep.links, rep.bytes.div_ceil(1024)),
            format!("Import it into the dev cluster and start the pod {} (user 10001, no token, no privilege, all capabilities dropped, memory 2 GiB, never pulls from the internet).", lp::pod_name(&spec.user, &spec.name)),
        ];
        if rep.setid_cleared > 0 {
            l.push(format!("{} file(s) lose their set-user-id or set-group-id bit.", rep.setid_cleared));
        }
        if !rep.dropped.is_empty() {
            l.push(format!("Left out (devices, pipes, sockets): {}", rep.dropped.iter().take(8).cloned().collect::<Vec<_>>().join(", ")));
        }
        Some(l.join("\n"))
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            "linuxapp_pod_start" => self.spec(args).err(),
            "linuxapp_pod_stop" => self.gate().err().or_else(|| if lp::valid_name(args["name"].as_str().unwrap_or("")) { None } else { Some("give the sandbox name (lower-case letters, digits and -)".into()) }),
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !TOOLS.contains(&name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == TOOLS[1] || name == TOOLS[2] { Some("a pod of a whole root changes the dev cluster, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
}

/// The real backend: the person's home, the managed dev cluster's `kubectl` and `k3s ctr`.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub policy: iyakki_sec::policy::Policy,
    pub user: String,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

struct DevKube(Arc<crate::services::Services>);
impl Kube for DevKube {
    fn run(&self, args: &[String]) -> crate::wsexec::Out {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        self.0.k3sdev.kubectl(&a)
    }
}

struct DevCtr(Arc<crate::services::Services>);
impl Importer for DevCtr {
    fn import(&self, file: &std::path::Path) -> Result<(), String> {
        let o = self.0.k3sdev.ctr(&["images", "import", &file.display().to_string()]);
        if o.ok() { Ok(()) } else { Err(o.combined().lines().last().unwrap_or("no answer").trim().to_string()) }
    }
}

impl Backend for RealBackend {
    fn is_admin(&self) -> bool {
        self.policy.manage_settings
    }
    fn user(&self) -> String {
        self.user.clone()
    }
    fn archive(&self, path: &str) -> Result<Vec<u8>, String> {
        let home = crate::scope::home_root_for_writes(&self.user, self.svc.dir());
        let p = std::path::Path::new(path);
        let full = if p.is_absolute() { p.to_path_buf() } else { home.join(p) };
        if full.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err("the path may not contain ..".into());
        }
        let real = std::fs::canonicalize(&full).map_err(|_| format!("{path} is not a file"))?;
        let home_real = std::fs::canonicalize(&home).unwrap_or(home);
        if !real.starts_with(&home_real) || !real.is_file() {
            return Err("the archive has to be a file inside your own home folder".into());
        }
        let len = std::fs::metadata(&real).map_err(|e| e.to_string())?.len();
        if len as usize > lp::MAX_ARCHIVE {
            return Err(format!("the archive is larger than {} MB", lp::MAX_ARCHIVE >> 20));
        }
        std::fs::read(&real).map_err(|e| e.to_string())
    }
    fn dev_cluster_running(&self) -> bool {
        self.svc.k3sdev.health().state == "running"
    }
    fn kube(&self) -> Arc<dyn Kube> {
        Arc::new(DevKube(self.svc.clone()))
    }
    fn importer(&self) -> Arc<dyn Importer> {
        Arc::new(DevCtr(self.svc.clone()))
    }
    fn work_dir(&self) -> PathBuf {
        self.svc.dir().join("linuxapps").join("pod-work")
    }
    fn arch(&self) -> String {
        if cfg!(target_arch = "aarch64") { "arm64".into() } else { "amd64".into() }
    }
    fn audit(&self, action: &str, detail: &str) {
        (self.audit)(action, detail)
    }
}

#[cfg(test)]
#[path = "linuxpodtools_tests.rs"]
mod tests;
