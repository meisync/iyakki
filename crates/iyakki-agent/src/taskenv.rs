//! What a parallel task gets besides its own tree (F-IDE-10), kept as plain logic over small seams so it runs with fakes:
//!
//! * read-only reference to the main workspace ([`reference_read`], [`reference_list`], [`ReferenceTools`]: `main_read`, `main_list`);
//! * a namespace of its own, created when the task is made and deployed into only by the task's own tool
//!   ([`TaskCluster`], [`provision`], [`check_manifest`], `task_deploy` in [`ReferenceTools`], which asks every time);
//! * dependency folders shared with the main workspace by symbolic link ([`share_dependency_folders`]);
//! * the question before the IDE closes with work still going ([`close_question`]);
//! * the uncommitted changes of the main workspace, listed and copied into a task on request ([`uncommitted_in_main`], [`bring_uncommitted`]);
//! * the model a task uses ([`effective_model`]).

pub use iyakki_shared::tasks::{close_question};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::tasks::Task;

/// A file read for reference is cut at this many bytes.
pub const REFERENCE_MAX_BYTES: usize = 200_000;
/// A listing shows at most this many entries.
pub const REFERENCE_MAX_ENTRIES: usize = 500;
/// Folders a task shares with the main workspace when the main one has them and git ignores them.
pub const SHARED_FOLDERS: [&str; 5] = ["node_modules", "target", ".venv", "venv", "vendor"];

fn safe_rel(rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim();
    let p = Path::new(rel);
    if p.is_absolute() || rel.contains('\0') {
        return Err("give a path inside the main workspace (relative)".into());
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::Normal(n) => {
                if n == ".git" || n == ".iyakki" {
                    return Err("that folder is not part of the reference".into());
                }
                out.push(n)
            }
            std::path::Component::CurDir => {}
            _ => return Err("the path may not leave the main workspace".into()),
        }
    }
    Ok(out)
}

fn resolve(main: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = safe_rel(rel)?;
    let root = std::fs::canonicalize(main).map_err(|e| format!("the main workspace cannot be read: {e}"))?;
    let real = std::fs::canonicalize(root.join(&rel)).map_err(|_| format!("{} does not exist in the main workspace", rel.display()))?;
    if !real.starts_with(&root) {
        return Err("that link points outside the main workspace".into());
    }
    Ok(real)
}

/// Read a text file of the main workspace; it is never written. Binary files and big files are refused or cut.
pub fn reference_read(main: &Path, rel: &str) -> Result<String, String> {
    let real = resolve(main, rel)?;
    if !real.is_file() {
        return Err("that is not a file (use main_list)".into());
    }
    let bytes = std::fs::read(&real).map_err(|e| e.to_string())?;
    if bytes.contains(&0) {
        return Err("that file is not text".into());
    }
    let cut = bytes.len() > REFERENCE_MAX_BYTES;
    let mut end = bytes.len().min(REFERENCE_MAX_BYTES);
    let text = loop {
        match std::str::from_utf8(&bytes[..end]) {
            Ok(t) => break t.to_string(),
            Err(_) if end > 0 => end -= 1,
            Err(_) => break String::new(),
        }
    };
    Ok(if cut { format!("{text}\n[cut at {REFERENCE_MAX_BYTES} bytes]") } else { text })
}

/// The entries of a folder of the main workspace (`name/` for a folder), without `.git` and `.iyakki`.
pub fn reference_list(main: &Path, rel: &str) -> Result<Vec<String>, String> {
    let real = if rel.trim().is_empty() || rel.trim() == "." { std::fs::canonicalize(main).map_err(|e| e.to_string())? } else { resolve(main, rel)? };
    if !real.is_dir() {
        return Err("that is not a folder".into());
    }
    let mut v: Vec<String> = std::fs::read_dir(&real)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            if n == ".git" || n == ".iyakki" {
                return None;
            }
            Some(if e.path().is_dir() { format!("{n}/") } else { n })
        })
        .collect();
    v.sort();
    v.truncate(REFERENCE_MAX_ENTRIES);
    Ok(v)
}

// ---- the task's own namespace

/// What the cluster must do for a task. The shell supplies the real one (kubectl); tests a fake.
pub trait TaskCluster: Send + Sync {
    fn create_namespace(&self, name: &str) -> Result<(), String>;
    fn delete_namespace(&self, name: &str) -> Result<(), String>;
    /// Apply a manifest into the namespace; the answer is what the cluster said.
    fn apply(&self, namespace: &str, manifest: &str) -> Result<String, String>;
}

/// Kinds a task may not deploy: they act outside its namespace.
const CLUSTER_KINDS: [&str; 10] = ["Namespace", "ClusterRole", "ClusterRoleBinding", "CustomResourceDefinition", "PersistentVolume", "StorageClass", "Node", "MutatingWebhookConfiguration", "ValidatingWebhookConfiguration", "PriorityClass"];

/// A manifest a task may apply into `namespace`: no cluster-wide kinds and no other namespace named.
pub fn check_manifest(namespace: &str, manifest: &str) -> Result<(), String> {
    if namespace.is_empty() {
        return Err("this task has no namespace".into());
    }
    if manifest.trim().is_empty() {
        return Err("the manifest is empty".into());
    }
    for line in manifest.lines() {
        let t = line.trim();
        if let Some(k) = t.strip_prefix("kind:") {
            let k = k.trim().trim_matches(['"', '\'']);
            if CLUSTER_KINDS.contains(&k) {
                return Err(format!("a task may not deploy a {k}: it acts outside the task's own namespace"));
            }
        }
        if let Some(n) = line.strip_prefix("  namespace:") {
            let n = n.trim().trim_matches(['"', '\'']);
            if n != namespace {
                return Err(format!("the manifest names namespace {n}; a task deploys only into {namespace}"));
            }
        }
    }
    Ok(())
}

/// Create the task's namespace when the task is made; the answer is a note for the task.
pub fn provision(cluster: &dyn TaskCluster, task: &Task) -> Result<String, String> {
    if task.namespace.is_empty() {
        return Err("the task has no namespace name".into());
    }
    cluster.create_namespace(&task.namespace)?;
    Ok(format!("Namespace {} was created for this task; deploy only with task_deploy.", task.namespace))
}

/// Tear the namespace down (merged or discarded).
pub fn teardown(cluster: &dyn TaskCluster, task: &Task) -> Result<(), String> {
    if task.namespace.is_empty() {
        return Ok(());
    }
    cluster.delete_namespace(&task.namespace)
}

// ---- shared dependency folders

/// Link the dependency folders the main workspace has (and git ignores) into the task's tree, so the task does not install
/// them again. Returns the names linked. A folder the tree already has is left alone.
pub fn share_dependency_folders(main: &Path, tree: &Path) -> Vec<String> {
    let ignored = crate::workspace::gitignored_names(main);
    let mut out = Vec::new();
    for name in SHARED_FOLDERS {
        let (from, to) = (main.join(name), tree.join(name));
        if !ignored.iter().any(|i| i == name) || !from.is_dir() || to.symlink_metadata().is_ok() {
            continue;
        }
        #[cfg(unix)]
        if std::os::unix::fs::symlink(&from, &to).is_ok() {
            out.push(name.to_string());
        }
    }
    out
}

// ---- closing, uncommitted changes, model


/// Paths with uncommitted changes in the main workspace (`git status --porcelain`); empty outside git.
pub fn uncommitted_in_main(main: &Path) -> Vec<String> {
    let Ok(o) = std::process::Command::new("git").current_dir(main).args(["status", "--porcelain", "-uall"]).output() else { return vec![] };
    if !o.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.len() > 3).map(|l| l[3..].trim().trim_matches('"').to_string()).filter(|p| !p.starts_with(".iyakki/")).collect()
}

/// Copy the listed uncommitted files of the main workspace into the task's tree (deleted ones are removed there). Returns the count.
pub fn bring_uncommitted(main: &Path, tree: &Path, files: &[String]) -> Result<usize, String> {
    let mut n = 0;
    for f in files {
        let rel = safe_rel(f)?;
        let (from, to) = (main.join(&rel), tree.join(&rel));
        if from.is_file() {
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::copy(&from, &to).map_err(|e| format!("{}: {e}", rel.display()))?;
            n += 1;
        } else if !from.exists() && to.is_file() {
            std::fs::remove_file(&to).map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    Ok(n)
}

/// Copy the uncommitted files of the main workspace into a task that is not running (they overwrite the task's copies). The IDE's
/// `My changes` button and the assistant's `task_bring_changes` both come here.
pub fn bring_my_changes(mgr: &crate::tasks::Manager, t: &Task) -> Result<String, String> {
    if !matches!(t.status, crate::tasks::Status::Queued | crate::tasks::Status::Paused | crate::tasks::Status::Interrupted | crate::tasks::Status::Failed) {
        return Err("your changes can be brought in while the task is not running (pause or stop it first)".to_string());
    }
    let main = mgr.root().to_path_buf();
    let files = uncommitted_in_main(&main);
    if files.is_empty() {
        return Err("your workspace has no uncommitted changes".to_string());
    }
    let n = bring_uncommitted(&main, &t.tree, &files)?;
    Ok(format!("{n} uncommitted file(s) of your workspace were copied into the task; they overwrite its copies"))
}

/// The model a task works with: its own when set, else the chat's.
pub fn effective_model(task: &Task, chat_model: &str) -> String {
    if task.model.trim().is_empty() { chat_model.to_string() } else { task.model.trim().to_string() }
}

// ---- the tools a task sees

/// Adds `main_read`, `main_list` (read-only, safe) and, with a cluster, `task_deploy` (asks every time) to a task's tools.
pub struct ReferenceTools {
    inner: Arc<dyn Tools>,
    main: PathBuf,
    namespace: String,
    cluster: Option<Arc<dyn TaskCluster>>,
}

const OWN: [&str; 3] = ["main_read", "main_list", "task_deploy"];

impl ReferenceTools {
    pub fn new(inner: Arc<dyn Tools>, main: &Path, namespace: &str, cluster: Option<Arc<dyn TaskCluster>>) -> ReferenceTools {
        ReferenceTools { inner, main: main.into(), namespace: namespace.into(), cluster }
    }
    fn own(&self, n: &str) -> bool {
        OWN.contains(&n) && (n != "task_deploy" || self.cluster.is_some())
    }
}

impl Tools for ReferenceTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.push(ToolDef { spec: ToolSpec { name: "main_read".into(), description: "Read a text file of the main workspace (the person's own files, outside your working folder). Read-only: you cannot change it.".into(), schema: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: "main_list".into(), description: "List a folder of the main workspace (read-only).".into(), schema: json!({"type":"object","properties":{"path":{"type":"string","description":"Relative folder; empty for the top."}}}) }, risk: Risk::Safe, untrusted: false });
        if self.cluster.is_some() {
            v.push(ToolDef { spec: ToolSpec { name: "task_deploy".into(), description: format!("Apply a Kubernetes manifest into this task's own namespace ({}) and nowhere else. The person approves every deploy.", self.namespace), schema: json!({"type":"object","properties":{"manifest":{"type":"string"}},"required":["manifest"]}) }, risk: Risk::Ask, untrusted: false });
        }
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match name {
            "main_read" => reference_read(&self.main, &s("path")),
            "main_list" => reference_list(&self.main, &s("path")).map(|v| if v.is_empty() { "(empty)".into() } else { v.join("\n") }),
            "task_deploy" if self.cluster.is_some() => {
                let m = s("manifest");
                check_manifest(&self.namespace, &m)?;
                self.cluster.as_ref().map_or(Err("no cluster".into()), |c| c.apply(&self.namespace, &m))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            "main_read" => format!("Read {} in the main workspace", args.get("path").and_then(Value::as_str).unwrap_or("")),
            "main_list" => "List a folder of the main workspace".into(),
            "task_deploy" if self.cluster.is_some() => format!("Deploy a manifest into the task's namespace {}", self.namespace),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name == "task_deploy" && self.cluster.is_some() {
            return args.get("manifest").and_then(Value::as_str).map(|m| crate::agent::truncate(m, 2000));
        }
        if self.own(name) { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == "task_deploy" && self.cluster.is_some() {
            return check_manifest(&self.namespace, args.get("manifest").and_then(Value::as_str).unwrap_or("")).err();
        }
        if self.own(name) { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !self.own(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != "task_deploy" && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == "task_deploy" { Some("a task's deploy asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
