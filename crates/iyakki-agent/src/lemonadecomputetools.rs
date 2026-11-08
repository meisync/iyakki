//! The lead assistant's tools for where Lemonade runs its models: `lemonade_compute_status` reads the choice, `lemonade_compute_set` changes it
//! (GPU is the default; CPU takes the GPU away from the Lemonade pod, which restarts). Changing it asks every time.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::gpu_steps::Compute;

pub const PROMPT_RULE: &str = "\nLEMONADE COMPUTE: lemonade_compute_status says whether Lemonade runs on the GPU (the default) or the CPU. lemonade_compute_set changes it (compute = gpu or cpu); it asks the person, and Lemonade restarts.";

pub const READ_TOOL: &str = "lemonade_compute_status";
pub const WRITE_TOOL: &str = "lemonade_compute_set";

pub fn is_tool(n: &str) -> bool {
    n == READ_TOOL || n == WRITE_TOOL
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    fn current(&self) -> Compute;
    /// Keep the choice and apply it to the cluster; the plain result line.
    fn set(&self, c: Compute) -> Result<String, String>;
}

pub struct LemonadeComputeTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

fn parse(args: &Value) -> Result<Compute, String> {
    let w = args.get("compute").and_then(Value::as_str).unwrap_or("");
    Compute::parse(w).ok_or_else(|| format!("`{w}` is not a choice; use gpu or cpu"))
}

impl LemonadeComputeTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> LemonadeComputeTools {
        LemonadeComputeTools { inner, backend }
    }
}

impl Tools for LemonadeComputeTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "Say whether Lemonade runs its models on the GPU or the CPU.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: WRITE_TOOL.into(),
                description: "Choose where Lemonade runs its models: gpu (default) or cpu. The person approves; the Lemonade pod restarts. With gpu and no GPU step installed, Lemonade stays on the CPU until one is.".into(),
                schema: json!({"type": "object", "properties": {"compute": {"type": "string", "enum": ["gpu", "cpu"]}}, "required": ["compute"]}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => Ok(format!("Lemonade is set to run on the {}.", self.backend.current().word().to_uppercase())),
            WRITE_TOOL => self.backend.set(parse(args)?),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            WRITE_TOOL => match parse(args) {
                Ok(c) => format!("Run Lemonade on the {} (it restarts)", c.word().to_uppercase()),
                Err(e) => format!("{name} (invalid: {e})"),
            },
            READ_TOOL => "Show where Lemonade runs".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == WRITE_TOOL { parse(args).err() } else { self.inner.refusal(name, args) }
    }
    fn pre_approved(&self, name: &str) -> bool {
        !is_tool(name) && self.inner.pre_approved(name)
    }
    fn approved_always(&self, name: &str) -> bool {
        name != WRITE_TOOL && self.inner.approved_always(name)
    }
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if is_tool(name) { None } else { self.inner.preview(name, args) }
    }
    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == WRITE_TOOL { Some("moving Lemonade between the GPU and the CPU restarts it, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

/// The real backend: this machine's data folder and cluster.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub user: String,
}

impl Backend for RealBackend {
    fn current(&self) -> Compute {
        crate::gpu_steps::lemonade_compute(self.svc.dir())
    }
    fn set(&self, c: Compute) -> Result<String, String> {
        crate::gpu_steps::set_lemonade_compute(self.svc.dir(), c)?;
        let mut env = crate::quickinstall::RealEnv::new(self.svc.dir(), &self.user);
        crate::gpu_steps::apply_lemonade_compute(&mut env, c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake(Mutex<Compute>);
    impl Backend for Fake {
        fn current(&self) -> Compute {
            *self.0.lock().unwrap()
        }
        fn set(&self, c: Compute) -> Result<String, String> {
            *self.0.lock().unwrap() = c;
            Ok(format!("now {}", c.word()))
        }
    }
    struct Nothing;
    impl Tools for Nothing {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, n: &str, _: &Value) -> Result<String, String> {
            Err(format!("no {n}"))
        }
        fn describe(&self, n: &str, _: &Value) -> String {
            n.into()
        }
    }

    #[test]
    fn the_status_is_free_the_change_asks_and_a_wrong_word_is_refused() {
        let t = LemonadeComputeTools::new(Arc::new(Nothing), Arc::new(Fake(Mutex::new(Compute::Gpu))));
        let defs = t.defs();
        assert!(defs.iter().any(|d| d.spec.name == READ_TOOL && d.risk == Risk::Safe));
        assert!(defs.iter().any(|d| d.spec.name == WRITE_TOOL && d.risk == Risk::Ask));
        assert!(t.call(READ_TOOL, &json!({})).unwrap().contains("GPU"));
        assert!(t.refusal(WRITE_TOOL, &json!({"compute": "tpu"})).is_some());
        assert_eq!(t.call(WRITE_TOOL, &json!({"compute": "cpu"})).unwrap(), "now cpu");
        assert!(t.call(READ_TOOL, &json!({})).unwrap().contains("CPU"));
        assert!(!t.pre_approved(WRITE_TOOL) && t.grant_blocker(WRITE_TOOL, &json!({})).is_some());
    }
}
