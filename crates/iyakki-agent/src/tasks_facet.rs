//! The real task manager behind `iyakki_agent_api::TaskMgr` (F-SPLIT-03).

use std::path::Path;
use std::sync::Arc;

use iyakki_agent_api::{TaskCluster as ApiCluster, TaskHooksIn, TaskMgr, TaskRunnerIn, TasksFacet};
use iyakki_shared::tasks::{Approval, AutoAssign, ChatLine, MergeCheck, NewTask, Runner, Task};

use crate::tasks::{autopilot_runner, Hooks, Manager, RunnerParts};

/// The api's cluster as the manager's.
struct ClusterAdapter(Arc<dyn ApiCluster>);
impl crate::taskenv::TaskCluster for ClusterAdapter {
    fn create_namespace(&self, name: &str) -> Result<(), String> {
        self.0.create_namespace(name)
    }
    fn delete_namespace(&self, name: &str) -> Result<(), String> {
        self.0.delete_namespace(name)
    }
    fn apply(&self, namespace: &str, manifest: &str) -> Result<String, String> {
        self.0.apply(namespace, manifest)
    }
}

/// The manager's cluster as the api's.
struct ClusterOut(Arc<dyn crate::taskenv::TaskCluster>);
impl ApiCluster for ClusterOut {
    fn create_namespace(&self, name: &str) -> Result<(), String> {
        self.0.create_namespace(name)
    }
    fn delete_namespace(&self, name: &str) -> Result<(), String> {
        self.0.delete_namespace(name)
    }
    fn apply(&self, namespace: &str, manifest: &str) -> Result<String, String> {
        self.0.apply(namespace, manifest)
    }
}

fn cluster_in(c: Option<Arc<dyn ApiCluster>>) -> Option<Arc<dyn crate::taskenv::TaskCluster>> {
    c.map(|c| Arc::new(ClusterAdapter(c)) as Arc<dyn crate::taskenv::TaskCluster>)
}

impl TaskMgr for Manager {
    fn concurrency(&self) -> usize {
        Manager::concurrency(self)
    }
    fn set_concurrency(&self, n: usize) {
        Manager::set_concurrency(self, n)
    }
    fn approvals(&self) -> Vec<Approval> {
        Manager::approvals(self)
    }
    fn log(&self, id: &str) -> Vec<String> {
        Manager::log(self, id)
    }
    fn stop(&self, id: &str) -> Result<(), String> {
        Manager::stop(self, id)
    }
    fn shutdown(&self) {
        Manager::shutdown(self)
    }
    fn resume(&self, id: &str, message: &str) -> Result<(), String> {
        Manager::resume(self, id, message)
    }
    fn pause(&self, id: &str) -> Result<(), String> {
        Manager::pause(self, id)
    }
    fn merge_check(&self, id: &str) -> Result<MergeCheck, String> {
        Manager::merge_check(self, id)
    }
    fn counts(&self) -> (usize, usize, usize) {
        Manager::counts(self)
    }
    fn chat(&self, id: &str) -> Vec<ChatLine> {
        Manager::chat(self, id)
    }
    fn spawn(&self, n: NewTask) -> Result<String, String> {
        Manager::spawn(self, n)
    }
    fn set_auto_assign(&self, a: AutoAssign) {
        Manager::set_auto_assign(self, a)
    }
    fn run_tests(&self, id: &str, program: &str, args: &[String]) -> Result<String, String> {
        Manager::run_tests(self, id, program, args)
    }
    fn retry(&self, id: &str) -> Result<(), String> {
        Manager::retry(self, id)
    }
    fn rebase(&self, id: &str) -> Result<String, String> {
        Manager::rebase(self, id)
    }
    fn poll(&self) {
        Manager::poll(self)
    }
    fn mode(&self, id: &str) -> Option<iyakki_shared::livemode::Mode> {
        Manager::mode(self, id)
    }
    fn merge(&self, id: &str) -> Result<String, String> {
        Manager::merge(self, id)
    }
    fn list(&self) -> Vec<Task> {
        Manager::list(self)
    }
    fn keep(&self, id: &str) -> Result<String, String> {
        Manager::keep(self, id)
    }
    fn guide(&self, id: &str, message: &str) -> Result<(), String> {
        Manager::guide(self, id, message)
    }
    fn discard(&self, id: &str) -> Result<(), String> {
        Manager::discard(self, id)
    }
    fn auto_assign(&self) -> AutoAssign {
        Manager::auto_assign(self)
    }
    fn diff(&self, id: &str) -> Result<String, String> {
        Manager::diff(self, id)
    }
    fn answer(&self, id: &str, call_id: &str, v: iyakki_shared::agent::Verdict) -> Result<(), String> {
        Manager::answer(self, id, call_id, v)
    }
    fn cycle_mode(&self, id: &str, by: &str) -> Result<iyakki_shared::livemode::Mode, String> {
        Manager::cycle_mode(self, id, by)
    }
    fn effective_concurrency(&self) -> usize {
        Manager::effective_concurrency(self)
    }
    fn get(&self, id: &str) -> Option<Task> {
        Manager::get(self, id)
    }
    fn bring_my_changes(&self, t: &Task) -> Result<String, String> {
        crate::taskenv::bring_my_changes(self, t)
    }
    fn agent_manager_any(&self) -> Arc<dyn std::any::Any + Send + Sync> {
        Arc::new(self.clone())
    }
    fn owner_of(&self, row: &str) -> Option<String> {
        Manager::owner_of(self, row)
    }
    fn waiting_on(&self, id: &str) -> Vec<String> {
        Manager::waiting_on(self, id)
    }
    fn set_memory_source(&self, f: Option<Arc<dyn Fn() -> Option<u64> + Send + Sync>>) {
        Manager::set_memory_source(self, f)
    }
    fn set_runner(&self, r: Runner) {
        Manager::set_runner(self, r)
    }
    fn set_hooks(&self, h: TaskHooksIn) {
        Manager::set_hooks(self, Hooks { audit: h.audit, notify: h.notify, cleanup: None, record: h.record, cluster: cluster_in(h.cluster) })
    }
}

struct RealTasks {
    svc: Arc<crate::services::Services>,
}

impl TasksFacet for RealTasks {
    fn open(&self, root: &Path, features: &Path) -> Option<Arc<dyn TaskMgr>> {
        Some(Arc::new(Manager::open(root, features)))
    }
    fn wrap_task_tools(&self, inner: Arc<dyn iyakki_shared::agent::Tools>, mgr: &Arc<dyn TaskMgr>, default_agent: &str, default_model: &str) -> Arc<dyn iyakki_shared::agent::Tools> {
        match mgr.agent_manager_any().downcast::<Manager>() {
            Ok(m) => Arc::new(crate::tasktools::TaskTools::new(inner, (*m).clone(), default_agent, default_model)),
            Err(_) => inner,
        }
    }
    fn autopilot_runner(&self, p: TaskRunnerIn) -> Option<Runner> {
        Some(autopilot_runner(RunnerParts { backend: p.backend, tools: p.tools, limits: p.limits, assist: p.assist, executor: p.executor, history: None, audit: p.audit, cluster: cluster_in(p.cluster) }))
    }
    fn cluster(&self) -> Option<Arc<dyn ApiCluster>> {
        Some(Arc::new(ClusterOut(self.svc.cluster.clone() as Arc<dyn crate::taskenv::TaskCluster>)))
    }
}

impl crate::services::Services {
    /// The task managers over these services (`cx.services.tasks_facet()`).
    pub fn tasks_facet(self: &Arc<Self>) -> Arc<dyn TasksFacet> {
        Arc::new(RealTasks { svc: self.clone() })
    }
}
