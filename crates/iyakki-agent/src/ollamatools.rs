//! The lead assistant's tools for the Ollama item of Quick install (see `quickinstall_ollama.rs`): `ollama_status` says where Ollama runs
//! (the cluster or this machine's host) and what it may use; `ollama_install` installs it into the cluster, moves an Ollama of the host
//! into the cluster (the models are copied after the person agrees and only when there is room) or starts fresh in the cluster and keeps the
//! host models where they are. It asks on a card every time. Removing goes through `quick_uninstall` with the item id `ollama` (the data is
//! kept unless `delete_data` is true).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::quickinstall::ollama_cluster::{self as oc, Decision, HostInfo, Mode, Where};

pub const PROMPT_RULE: &str = "\nOLLAMA: ollama_status says where Ollama runs (in the cluster, or on this machine's host) and its memory and CPU limits. ollama_install asks the person on a card: how=cluster installs it in the cluster; for an Ollama that runs on the host, how=move copies its models into the cluster (only when there is room) and how=fresh starts empty in the cluster and keeps the host models where they are. The host Ollama is stopped only after the cluster one answers and nothing is deleted from the host. To remove Ollama use quick_uninstall with item ollama (data kept unless asked).";

pub const READ_TOOL: &str = "ollama_status";
pub const WRITE_TOOL: &str = "ollama_install";

pub fn is_ollama_tool(n: &str) -> bool {
    n == READ_TOOL || n == WRITE_TOOL
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    fn where_runs(&self) -> Where;
    fn host_info(&self) -> HostInfo;
    /// Start the Quick install job for the chosen way; the plain result line.
    fn install(&self, how: Option<Mode>) -> Result<String, String>;
}

pub struct OllamaTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

/// The way asked for: `None` is a plain install in the cluster.
fn how(args: &Value) -> Result<Option<Mode>, String> {
    match args.get("how").and_then(Value::as_str).unwrap_or("cluster") {
        "cluster" => Ok(None),
        "move" => Ok(Some(Mode::Move)),
        "fresh" => Ok(Some(Mode::Fresh)),
        other => Err(format!("how must be cluster, move or fresh, not `{other}`")),
    }
}

impl OllamaTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> OllamaTools {
        OllamaTools { inner, backend }
    }

    /// The card, or the plain reason the call cannot be made.
    fn card(&self, args: &Value) -> Result<String, String> {
        let how = how(args)?;
        match (self.backend.where_runs(), how) {
            (Where::Cluster(_), _) => Err("Ollama already runs in the cluster; nothing to install. To remove it use quick_uninstall with item ollama.".into()),
            (Where::Host(_), None) => Err("Ollama runs on this machine's host. Ask again with how=move (copy its models into the cluster) or how=fresh (start empty in the cluster, keep the host models where they are).".into()),
            (Where::Nowhere, Some(_)) => Err("There is no Ollama on this machine's host to move; use how=cluster.".into()),
            (Where::Nowhere, None) => Ok(format!("Install Ollama in the cluster (namespace {}, ClusterIP, 127.0.0.1:{} on this machine, {} GB volume). The cluster pulls the image from Docker Hub (MIT licence). It is added to Models as ollama-local.", oc::NS, oc::LOCAL_PORT, oc::STORAGE_GB)),
            (Where::Host(_), Some(mode)) => {
                let h = self.backend.host_info();
                let d = oc::decide(&h);
                if mode == Mode::Move && d != Decision::Copy {
                    return Err(oc::review_lines(mode, &h, d).into_iter().find(|l| l.contains("too large") || l.contains("not enough room")).unwrap_or_else(|| "the models cannot be copied now; use how=fresh".into()));
                }
                Ok(oc::review_lines(mode, &h, d).join("\n"))
            }
        }
    }
}

impl Tools for OllamaTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_ollama_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "Say where Ollama runs (in the cluster or on this machine's host), its pod, and its CPU and memory requests and limits.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: WRITE_TOOL.into(),
                description: "Install Ollama in the cluster (Quick install), or move an Ollama of the host into it. The person approves a card every time.".into(),
                schema: json!({"type": "object", "properties": {
                    "how": {"type": "string", "enum": ["cluster", "move", "fresh"], "description": "cluster: a new install (no Ollama on the host); move: copy the host models into the cluster (only when there is room); fresh: start empty in the cluster and keep the host models where they are. The host Ollama is stopped only after the cluster one answers; nothing is deleted."}
                }}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => Ok(oc::status_line(&self.backend.where_runs())),
            WRITE_TOOL => {
                self.card(args)?;
                self.backend.install(how(args)?)
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            WRITE_TOOL => match self.card(args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            },
            READ_TOOL => "Show where Ollama runs".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card(args).ok(),
            READ_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card(args).err(),
            READ_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_ollama_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != WRITE_TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == WRITE_TOOL { Some("installing or moving Ollama asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

/// The real backend: this machine's Quick install.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub user: String,
}

impl Backend for RealBackend {
    fn where_runs(&self) -> Where {
        let host = self.svc.clis.installed().into_iter().find(|i| i.id == "ollama").map(|i| i.version);
        oc::where_runs(self.svc.dir(), host)
    }
    fn host_info(&self) -> HostInfo {
        oc::real_host_info(self.svc.dir())
    }
    fn install(&self, how: Option<Mode>) -> Result<String, String> {
        let l = crate::quickinstall::Launch::this_program(self.svc.dir())?;
        if let Some(m) = how {
            oc::write_mode(self.svc.dir(), m)?;
        }
        let present = |i: crate::quickinstall::Item| matches!(crate::quickinstall::presence(&self.svc, i), crate::quickinstall::Presence::Installed(_)) && i != crate::quickinstall::Item::Ollama;
        let jobs = crate::quickinstall::start(&self.svc.jobs, &self.user, crate::quickinstall::Item::Ollama, &present, &l)?;
        Ok(format!("started {} job(s) for Ollama in the cluster; progress is beside the clock in the top bar and in the Store's Setup tab", jobs.len()))
    }
}
