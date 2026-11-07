//! The lead assistant's tools for the object storage items of Quick install (SeaweedFS, RustFS; see `quickinstall_objstore.rs`):
//! `object_storage_status` reads (no card); `object_storage_install` and `object_storage_remove` ask on a card, every time (hard ask:
//! installing pulls an image into the cluster, removing can delete data).
//!
//! * The assistant learns NAMES only: the vault entries that hold the keys and the record `s3.<name>`. A key is never read, sent or
//!   printed by these tools, and a call that carries something that looks like a key is refused without echoing it.
//! * The install starts the same Quick install job as the Store's Setup tab; the result says where its progress is.
//! * Remove keeps the data unless `delete_data` is true; the card says which.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::quickinstall::objstore::{self, Connection, Engine};

pub const PROMPT_RULE: &str = "\nOBJECT STORAGE: object_storage_status shows the S3-compatible stores of Quick install (SeaweedFS, RustFS): whether installed, the in-cluster endpoint, bucket and the NAMES of the vault entries that hold the keys (you never see a key). object_storage_install and object_storage_remove each ask the person. Never ask for or type an access key or secret key in the chat; the person's vault holds them. The endpoint works inside the cluster only.";

pub const WRITE_TOOLS: [&str; 2] = ["object_storage_install", "object_storage_remove"];
pub const READ_TOOL: &str = "object_storage_status";

pub fn is_object_storage_tool(n: &str) -> bool {
    n == READ_TOOL || WRITE_TOOLS.contains(&n)
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    /// The engine's install marker text, when installed.
    fn installed(&self, e: &Engine) -> Option<String>;
    /// The person's vault entry names (not values).
    fn vault_names(&self) -> Vec<String>;
    /// The record `s3.<name>` text when the person's vault is open and has it.
    fn record(&self, e: &Engine) -> Option<String>;
    /// Start the Quick install job; the plain result line.
    fn install(&self, e: &Engine) -> Result<String, String>;
    /// Remove (keeping the data unless `delete_data`); the plain result line.
    fn remove(&self, e: &Engine, delete_data: bool) -> Result<String, String>;
}

pub struct ObjectStorageTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

/// Anything that looks like key material (an AWS-style access key id, or a long unbroken token).
fn looks_like_key(v: &str) -> bool {
    let t = v.trim();
    (t.len() == 20 && t.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())) || (t.len() >= 32 && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')))
}

impl ObjectStorageTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> ObjectStorageTools {
        ObjectStorageTools { inner, backend }
    }

    fn engine(&self, args: &Value) -> Result<&'static Engine, String> {
        for (_k, v) in args.as_object().into_iter().flatten() {
            if let Some(s) = v.as_str() {
                if looks_like_key(s) {
                    return Err("never send a key through a tool; the keys stay in the person's vault".into());
                }
            }
        }
        let w = args.get("engine").and_then(Value::as_str).unwrap_or("");
        objstore::engine_named(w).ok_or_else(|| "name the store: seaweedfs or rustfs".to_string())
    }

    fn card(&self, name: &str, args: &Value) -> Result<String, String> {
        let e = self.engine(args)?;
        match name {
            "object_storage_install" => Ok(format!("Install {} (S3 object storage, {}) in the cluster. The cluster pulls its image from Docker Hub; the access key and secret key are made for you and kept in your vault. ClusterIP only, {} volume.", e.title, e.item.licence().split('.').next().unwrap_or(""), e.storage)),
            _ => {
                let del = args.get("delete_data").and_then(Value::as_bool).unwrap_or(false);
                Ok(if del {
                    format!("Remove {} AND DELETE ITS DATA: the volume {}, the Secret and your vault entries {}, {} and {}. This cannot be undone.", e.title, e.claim(), e.access_vault(), e.secret_vault(), e.record_name())
                } else {
                    format!("Remove {}. The data ({}) and the keys are kept for a later install.", e.title, e.claim())
                })
            }
        }
    }

    fn status_line(&self, e: &Engine) -> String {
        let names = self.backend.vault_names();
        let has = |n: &str| if names.iter().any(|x| x == n) { "in the vault" } else { "not in the vault" };
        match self.backend.installed(e) {
            None => format!("{}: not installed (object_storage_install {} asks the person)", e.title, e.name),
            Some(d) => {
                let rec = self.backend.record(e).and_then(|t| Connection::from_text(&t).ok());
                let (ep, bucket) = rec.map_or((e.endpoint(), objstore::BUCKET.to_string()), |c| (c.endpoint, c.bucket));
                format!("{}: installed ({d}). Endpoint {ep} (inside the cluster only), bucket {bucket}, path style, region {}. Key names: {} ({}), {} ({}); record {} ({}).", e.title, objstore::REGION, e.access_vault(), has(&e.access_vault()), e.secret_vault(), has(&e.secret_vault()), e.record_name(), has(&e.record_name()))
            }
        }
    }
}

impl Tools for ObjectStorageTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_object_storage_tool(&d.spec.name));
        let engine = json!({"type": "string", "enum": ["seaweedfs", "rustfs"], "description": "seaweedfs (mature, the default choice) or rustfs (young, Rust)."});
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "Show the S3 object stores of Quick install: installed or not, in-cluster endpoint, bucket, and the NAMES of the vault entries that hold the keys. Never returns a key.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: "object_storage_install".into(), description: "Install an S3-compatible object store in the cluster (Quick install: SeaweedFS or RustFS, both Apache-2.0). The person approves a card; the keys are generated into their vault.".into(), schema: json!({"type": "object", "properties": {"engine": engine}, "required": ["engine"]}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: "object_storage_remove".into(), description: "Remove an object store from the cluster. The data is kept unless delete_data is true. The person approves a card that says which.".into(), schema: json!({"type": "object", "properties": {"engine": engine, "delete_data": {"type": "boolean", "description": "true deletes the volume, the Secret and the vault entries; default false keeps them."}}, "required": ["engine"]}) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => Ok(objstore::ENGINES.iter().map(|e| self.status_line(e)).collect::<Vec<_>>().join("\n")),
            "object_storage_install" => {
                let e = self.engine(args)?;
                self.backend.install(e)
            }
            "object_storage_remove" => {
                let e = self.engine(args)?;
                self.backend.remove(e, args.get("delete_data").and_then(Value::as_bool).unwrap_or(false))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if WRITE_TOOLS.contains(&name) {
            return match self.card(name, args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        if name == READ_TOOL { "Show the object storage".into() } else { self.inner.describe(name, args) }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).ok();
        }
        if name == READ_TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).err();
        }
        if name == READ_TOOL { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_object_storage_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !WRITE_TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) { Some("installing or removing object storage asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

/// The real backend: the Quick install job, the cluster, the person's vault.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub user: String,
}

impl Backend for RealBackend {
    fn installed(&self, e: &Engine) -> Option<String> {
        crate::quickinstall::marked(self.svc.dir(), e.item)
    }
    fn vault_names(&self) -> Vec<String> {
        self.svc.user_names(&self.user).unwrap_or_default()
    }
    fn record(&self, e: &Engine) -> Option<String> {
        self.svc.user_get(&self.user, &e.record_name()).ok().map(|z| z.to_string())
    }
    fn install(&self, e: &Engine) -> Result<String, String> {
        let l = crate::quickinstall::Launch::this_program(self.svc.dir())?;
        let present = |i: crate::quickinstall::Item| matches!(crate::quickinstall::presence(&self.svc, i), crate::quickinstall::Presence::Installed(_));
        let jobs = crate::quickinstall::start(&self.svc.jobs, &self.user, e.item, &present, &l)?;
        Ok(format!("started {} job(s) for {}; progress is beside the clock in the top bar and in the Store's Setup tab. The keys will be put into the vault when it is unlocked.", jobs.len(), e.title))
    }
    fn remove(&self, e: &Engine, delete_data: bool) -> Result<String, String> {
        struct Env<'a>(&'a RealBackend);
        impl objstore::RemoveEnv for Env<'_> {
            fn kubectl(&mut self, args: &[&str]) -> Result<String, String> {
                let o = self.0.svc.k3sdev.kubectl(args);
                if o.code == Some(0) { Ok(o.stdout) } else { Err(o.combined().trim().to_string()) }
            }
            fn unmark(&mut self, item: crate::quickinstall::Item) {
                crate::quickinstall::unmark(self.0.svc.dir(), item);
            }
            fn vault_delete(&mut self, name: &str) -> Result<(), String> {
                if !self.0.vault_names().iter().any(|n| n == name) {
                    return Ok(());
                }
                self.0.svc.user_delete(&self.0.user, name).map_err(|e| e.to_string())
            }
        }
        objstore::remove(e, &mut Env(self), delete_data)
    }
}
