//! Hand-written facets of the agent for the UI (F-SPLIT-03, alpha.5 item 0a): what the generated functions cannot say because it is a
//! handle with state or a family of calls that belong together. Each facet is a narrow trait with default answers ("unavailable"), a
//! real implementation in `iyakki-agent` (`agentfacets.rs`) and a fake here, so a UI test never needs the agent.
//! `AgentFacets` is a supertrait of `AgentApi`, so `cx.agent.exec()` and `cx.agent.workspace_locks()` need nothing more.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use iyakki_sec::policy::Policy;
use iyakki_shared::devmode::grants::ShellRequest;
use iyakki_shared::devmode::input::InputEv;
use iyakki_shared::workspace::{LockInfo, LockState};
use iyakki_shared::wsexec::{FakeRunner, Out, Runner};

/// Starting programs the way the IDE and the cluster views do: argument arrays, a cut-down environment, a time and an output cap.
pub trait ExecFacet: Send + Sync {
    /// The real runner (it also knows about the managed k3s and vault kubeconfigs); the default is a runner that finds no program.
    fn system(&self) -> Arc<dyn Runner> {
        Arc::new(FakeRunner::new(&[]))
    }
    /// Run `argv` in `cwd` once.
    fn run_argv(&self, _cwd: &Path, _argv: &[String], _env: &[(String, String)], _timeout: Duration, _cap: usize) -> Out {
        Out { stderr: "unavailable: this core does not start programs".into(), ..Default::default() }
    }
    /// Is a program of this name on `PATH`?
    fn on_path(&self, _name: &str) -> bool {
        false
    }
    /// `git init` in a new folder.
    fn git_init(&self, _dir: &Path) -> Result<(), String> {
        Err("unavailable: this core does not start programs".into())
    }
}

/// The advisory lock of a workspace (two windows must not edit one unaware). Reading and releasing a lock is pure and lives in
/// `iyakki_shared::workspace`; deciding whether the holder's process still runs needs the machine, so it is asked here.
pub trait WorkspaceLocks: Send + Sync {
    /// Take the lock unless a live one belongs to another session.
    fn acquire(&self, _root: &Path, _user: &str, _session: &str, _now: u64) -> LockState {
        LockState::Mine
    }
    /// Keep a held lock fresh; `false` when somebody else holds it now.
    fn refresh(&self, _root: &Path, _user: &str, _session: &str, _now: u64) -> bool {
        true
    }
    /// Take the lock over from another session.
    fn take_over(&self, _root: &Path, _user: &str, _session: &str, _now: u64) {}
    /// The lock of `root` if a live session holds it.
    fn live(&self, _root: &Path, _now: u64) -> Option<LockInfo> {
        None
    }
}


/// Developer mode as the shell and the Doctor see it: the sign in the top bar, the question a paired computer asks to run commands
/// (the approval is a person's answer on the device), the hand-off of what a computer queued (opened apps, keys), the update that did not
/// finish. The state machine (pairing, keys, scopes, the listener) stays in the agent; the Settings page of developer mode has its own handle.
pub trait DevModeFacet: Send + Sync {
    fn is_on(&self, _data: &Path) -> bool {
        false
    }
    /// The sign in the top bar: `DEV`, `DEV \u{b7} pairing`, `DEV \u{b7} SHELL ACCESS ...`, or the interrupted-update line (which wins); `None` when nothing is on.
    fn top_sign(&self, _data: &Path) -> Option<String> {
        None
    }
    /// Switch off later: the end time (unix seconds) after `hours` more hours.
    fn extend(&self, _data: &Path, _policy: &Policy, _user: &str, _hours: u32) -> Result<i64, String> {
        Err("unavailable: this core does not serve developer mode".into())
    }
    /// The certificate fingerprint of the developer listener, once it exists.
    fn cert_fingerprint(&self, _data: &Path) -> Option<String> {
        None
    }
    /// A number that changes when a pushed build of the named developer app was installed.
    fn reload_stamp(&self, _data: &Path, _app: &str) -> u64 {
        0
    }
    /// One line of an app's console for `iyakki dev logs`.
    fn app_log(&self, _data: &Path, _app: &str, _line: &str) {}
    /// Requests of paired computers to run commands that wait for a person (empty while developer mode is off).
    fn shell_pending(&self, _data: &Path) -> Vec<ShellRequest> {
        Vec::new()
    }
    /// Approve a request for `minutes` (0: one session); the computer and the kind of grant.
    fn shell_approve(&self, _data: &Path, _policy: &Policy, _user: &str, _computer: &str, _minutes: u32) -> Result<(String, String), String> {
        Err("unavailable".into())
    }
    /// Approve the following requests of the computer by themselves for `hours`.
    fn shell_auto_on(&self, _data: &Path, _policy: &Policy, _user: &str, _computer: &str, _hours: u32) -> Result<(), String> {
        Err("unavailable".into())
    }
    fn shell_deny(&self, _data: &Path, _policy: &Policy, _user: &str, _computer: &str) -> Result<(), String> {
        Err("unavailable".into())
    }
    /// Apps a paired computer asked to open since the last call.
    fn take_open(&self, _data: &Path) -> Vec<String> {
        Vec::new()
    }
    /// Tell the input hand-off whether a password field has focus / an approval card is up (a computer's input is refused then).
    fn set_secret_focus(&self, _data: &Path, _on: bool) {}
    fn set_approval_card(&self, _data: &Path, _on: bool) {}
    /// Keys and taps a paired computer queued.
    fn take_input(&self, _data: &Path) -> Vec<InputEv> {
        Vec::new()
    }
    /// A system update was started and never finished (the boot menu keeps the old version).
    fn update_interrupted(&self, _data: &Path) -> bool {
        false
    }
    /// The sentence for it.
    fn update_interrupted_text(&self, _data: &Path) -> Option<String> {
        None
    }
    /// The last trial of an update fell back to the previous version.
    fn update_fell_back(&self, _data: &Path) -> bool {
        false
    }
    /// The first picture of this start was drawn: a standing restart grant may be used again, and a trial boot is confirmed. The line to log.
    fn boot_confirmed(&self, _data: &Path) -> Option<String> {
        None
    }
    /// The compatibility report as lines (peers are listed for administrators only).
    fn compat_lines(&self, _data: &Path, _admin: bool) -> Vec<String> {
        Vec::new()
    }
    /// A line with passwords and tokens taken out, for the startup screen.
    fn scrub(&self, line: &str) -> String {
        line.to_string()
    }
}

struct NoFacet;
impl ExecFacet for NoFacet {}
impl WorkspaceLocks for NoFacet {}
impl DevModeFacet for NoFacet {}

/// The facets, one accessor each. Every accessor has an "unavailable" default.
pub trait AgentFacets: Send + Sync + crate::facets_b1::AgentFacetsB1 + crate::facets_b2::AgentFacetsB2 + crate::facets_b4::AgentFacetsB4 + crate::facets_s6b::AgentFacetsS6B + crate::facets_b6::AgentFacetsB6 + crate::facets_b7::AgentFacetsB7 + crate::facets_b8::AgentFacetsB8 {
    /// True only for the real implementation (`AgentApiImpl`); the fake and the defaults answer false. The shell refuses to start on a handle
    /// that is not real, and the sweep tests check it (KI-devmode-off-2026-11-17: a fake handle won the first-use race and every call said "unavailable").
    fn is_real(&self) -> bool {
        false
    }
    fn exec(&self) -> &dyn ExecFacet {
        &NoFacet
    }
    fn workspace_locks(&self) -> &dyn WorkspaceLocks {
        &NoFacet
    }
    fn devmode(&self) -> &dyn DevModeFacet {
        &NoFacet
    }
    /// The facets of split bucket b0 (see `facets_b0.rs`).
    fn b0(&self) -> &dyn crate::facets_b0::B0Facets {
        &crate::facets_b0::NoB0
    }
}

impl AgentFacets for crate::FakeAgentApi {}
