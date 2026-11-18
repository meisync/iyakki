//! The parallel tasks of the IDE as the UI uses them: one trait over the agent's task manager, so the UI holds `Arc<dyn TaskMgr>` and never names
//! `Manager`. The data (`Task`, `RunSpec`, `RunRecord`, the budgets) is in `iyakki_shared::tasks`.

use std::path::Path;
use std::sync::Arc;

use iyakki_shared::agent::{RunLimits, Tools};
use iyakki_shared::assistrun::Assist;
use iyakki_shared::tasks::{Approval, AutoAssign, ChatLine, MergeCheck, NewTask, RunRecord, RunSpec, Runner, Task};

pub const UNAVAILABLE_TASKS: &str = "unavailable: this core does not serve this call";

/// A task's own namespace in the machine's cluster (made with the task, deleted with it).
pub trait TaskCluster: Send + Sync {
    fn create_namespace(&self, name: &str) -> Result<(), String>;
    fn delete_namespace(&self, name: &str) -> Result<(), String>;
    /// Apply a manifest into the namespace; the answer is what the cluster said.
    fn apply(&self, namespace: &str, manifest: &str) -> Result<String, String>;
}

/// What the task manager tells the person: the audit log, the inbox, the run log.
#[derive(Clone, Default)]
pub struct TaskHooksIn {
    pub audit: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
    pub notify: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
    pub record: Option<Arc<dyn Fn(RunRecord) + Send + Sync>>,
    pub cluster: Option<Arc<dyn TaskCluster>>,
}

/// What a runner of tasks is made from (the same machinery as Autopilot, with the person's own tools narrowed to the task's tree).
pub struct TaskRunnerIn {
    pub backend: Arc<dyn iyakki_shared::chat::ChatBackend>,
    pub tools: Arc<dyn Fn(&RunSpec) -> Arc<dyn Tools> + Send + Sync>,
    pub limits: RunLimits,
    pub assist: Assist,
    pub executor: Option<Arc<dyn Fn(&RunSpec, Arc<dyn Tools>) -> Option<iyakki_shared::autopilot::Executor> + Send + Sync>>,
    pub audit: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
    pub cluster: Option<Arc<dyn TaskCluster>>,
}

/// The tasks of one workspace.
pub trait TaskMgr: Send + Sync {
    fn concurrency(&self) -> usize {
        0
    }
    fn set_concurrency(&self, _n: usize) {}
    fn approvals(&self) -> Vec<Approval> {
        Vec::new()
    }
    fn log(&self, _id: &str) -> Vec<String> {
        Vec::new()
    }
    fn stop(&self, _id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn shutdown(&self) {}
    fn resume(&self, _id: &str, _message: &str) -> Result<(), String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn pause(&self, _id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn merge_check(&self, _id: &str) -> Result<MergeCheck, String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn counts(&self) -> (usize, usize, usize) {
        (0, 0, 0)
    }
    fn chat(&self, _id: &str) -> Vec<ChatLine> {
        Vec::new()
    }
    fn spawn(&self, _n: NewTask) -> Result<String, String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn set_auto_assign(&self, _a: AutoAssign) {}
    fn run_tests(&self, _id: &str, _program: &str, _args: &[String]) -> Result<String, String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn retry(&self, _id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn rebase(&self, _id: &str) -> Result<String, String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn poll(&self) {}
    fn mode(&self, _id: &str) -> Option<iyakki_shared::livemode::Mode> {
        None
    }
    fn merge(&self, _id: &str) -> Result<String, String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn list(&self) -> Vec<Task> {
        Vec::new()
    }
    fn keep(&self, _id: &str) -> Result<String, String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn guide(&self, _id: &str, _message: &str) -> Result<(), String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn discard(&self, _id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn auto_assign(&self) -> AutoAssign {
        AutoAssign::Off
    }
    fn diff(&self, _id: &str) -> Result<String, String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn answer(&self, _id: &str, _call_id: &str, _v: iyakki_shared::agent::Verdict) -> Result<(), String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn cycle_mode(&self, _id: &str, _by: &str) -> Result<iyakki_shared::livemode::Mode, String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    fn effective_concurrency(&self) -> usize {
        0
    }
    fn get(&self, _id: &str) -> Option<Task> {
        None
    }
    /// Bring the person's own changes into a task's tree (the task must not be running).
    fn bring_my_changes(&self, _t: &Task) -> Result<String, String> {
        Err(UNAVAILABLE_TASKS.into())
    }
    /// The agent's own manager as an opaque handle, for the agent-side tools that work on it.
    fn agent_manager_any(&self) -> Arc<dyn std::any::Any + Send + Sync> {
        Arc::new(())
    }
    fn owner_of(&self, _row: &str) -> Option<String> {
        None
    }
    fn waiting_on(&self, _id: &str) -> Vec<String> {
        Vec::new()
    }
    /// Where the free memory is read from (tests give a fixed number).
    fn set_memory_source(&self, _f: Option<Arc<dyn Fn() -> Option<u64> + Send + Sync>>) {}
    fn set_runner(&self, _r: Runner) {}
    fn set_hooks(&self, _h: TaskHooksIn) {}
}

/// The task managers of a core: open one for a workspace, make the runner of its tasks.
pub trait TasksFacet: Send + Sync {
    fn open(&self, _root: &Path, _features: &Path) -> Option<Arc<dyn TaskMgr>> {
        None
    }
    /// The task tools of the chat (`task_spawn`, ...) around `inner`, over this manager.
    fn wrap_task_tools(&self, inner: Arc<dyn Tools>, _mgr: &Arc<dyn TaskMgr>, _default_agent: &str, _default_model: &str) -> Arc<dyn Tools> {
        inner
    }
    fn autopilot_runner(&self, _parts: TaskRunnerIn) -> Option<Runner> {
        None
    }
    /// The cluster the tasks get their namespaces in (None: this core has no cluster).
    fn cluster(&self) -> Option<Arc<dyn TaskCluster>> {
        None
    }
}
