//! Pending work (the ledger and the waker): the chat and the voice assistant ask the core for a waker and a status line, and never
//! name the ledger, the feed or the jobs themselves (F-SPLIT-03). Hand-written: it needs the agent's services.

use std::sync::Arc;

/// A running watcher of the person's pending work: when something the assistant waited for has ended it says so, and gives the turn to start.
pub trait PendingWaker: Send {
    /// Something ended and the assistant has not been told (cheap: read every frame).
    fn ready(&self) -> bool;
    /// The turn to start now (call only when no run is in progress).
    fn take(&self) -> Option<iyakki_shared::pending::Wake>;
}

/// Where a chat's own tasks are read from (the jobs are the core's): the chat's task manager, handed over as an opaque handle that only the
/// real core knows how to read (the manager's type lives in the agent).
pub type TaskList = Arc<dyn crate::facets_tasks::TaskMgr>;

/// What the UI asks of the pending-work ledger.
pub trait AgentPending: Send + Sync {
    /// Start the waker for this person and workspace (`workspace` empty: no workspace). `None` when this core has no ledger.
    fn pending_waker(&self, _user: &str, _workspace: &str, _tasks: Option<TaskList>) -> Option<Box<dyn PendingWaker>> {
        None
    }
    /// The pending-work tools (wait_for, pending_list) around `inner`, over the person's ledger and where their jobs and tasks are read.
    #[allow(clippy::too_many_arguments)]
    fn wrap_pending_tools(&self, inner: Arc<dyn iyakki_shared::agent::Tools>, _user: &str, _workspace: &str, _tasks: Option<TaskList>, _policy: iyakki_sec::policy::Policy, _cwd: &std::path::Path, _audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn iyakki_shared::agent::Tools> {
        inner
    }
    /// The person's Waiting settings (auto-continue, separate thread, wakes per day, longest wait).
    fn pending_settings(&self, _user: &str) -> iyakki_shared::pending::Settings {
        Default::default()
    }
    /// Save the Waiting settings.
    fn pending_set_settings(&self, _user: &str, _s: iyakki_shared::pending::Settings) {}
    /// How many wakes are left today.
    fn pending_wakes_left(&self, _user: &str) -> u32 {
        0
    }
    /// The one line a chat strip shows for running and fresh work (`None`: nothing to say). Jobs are left out (the jobs strip has them).
    fn pending_strip_line(&self, _user: &str, _workspace: &str, _now_ms: u64) -> Option<String> {
        None
    }
}

/// A core with no ledger: nothing to wait for.
pub struct NoPending;
impl AgentPending for NoPending {}

/// The fake: a waker a test controls and a strip line it sets; every ask is recorded.
#[derive(Default)]
pub struct FakePending {
    pub settings: std::sync::Mutex<iyakki_shared::pending::Settings>,
    pub strip: std::sync::Mutex<Option<String>>,
    pub asked: std::sync::Mutex<Vec<String>>,
    pub wake: std::sync::Arc<std::sync::Mutex<Option<iyakki_shared::pending::Wake>>>,
}

struct FakeWaker(std::sync::Arc<std::sync::Mutex<Option<iyakki_shared::pending::Wake>>>);

impl PendingWaker for FakeWaker {
    fn ready(&self) -> bool {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
    }
    fn take(&self) -> Option<iyakki_shared::pending::Wake> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
    }
}

impl AgentPending for FakePending {
    fn pending_waker(&self, user: &str, workspace: &str, _tasks: Option<TaskList>) -> Option<Box<dyn PendingWaker>> {
        self.asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(format!("waker({user},{workspace})"));
        Some(Box::new(FakeWaker(self.wake.clone())))
    }
    fn pending_settings(&self, _user: &str) -> iyakki_shared::pending::Settings {
        self.settings.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
    fn pending_set_settings(&self, _user: &str, s: iyakki_shared::pending::Settings) {
        *self.settings.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = s;
    }
    fn pending_strip_line(&self, user: &str, workspace: &str, _now_ms: u64) -> Option<String> {
        self.asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(format!("strip({user},{workspace})"));
        self.strip.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}
