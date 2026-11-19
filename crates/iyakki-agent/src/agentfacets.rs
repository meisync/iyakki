//! The real implementation of the hand-written facets of `iyakki-agent-api` (F-SPLIT-03): `AgentApiImpl` answers them from this crate.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use iyakki_agent_api::{AgentFacets, DevModeFacet, ExecFacet, WorkspaceLocks};
use iyakki_sec::policy::Policy;
use iyakki_shared::devmode::grants::ShellRequest;
use iyakki_shared::devmode::input::InputEv;
use iyakki_shared::workspace::{LockInfo, LockState};
use iyakki_shared::wsexec::{Out, Runner};

use crate::agentapi_gen::AgentApiImpl;

pub struct RealExec;

impl ExecFacet for RealExec {
    fn system(&self) -> Arc<dyn Runner> {
        Arc::new(crate::wsexec::System)
    }
    fn run_argv(&self, cwd: &Path, argv: &[String], env: &[(String, String)], timeout: Duration, cap: usize) -> Out {
        crate::wsexec::run_argv(cwd, argv, env, timeout, cap)
    }
    fn on_path(&self, name: &str) -> bool {
        crate::wsexec::on_path(name)
    }
    fn git_init(&self, dir: &Path) -> Result<(), String> {
        crate::wsexec::git_init(dir)
    }
}

pub struct RealLocks;

impl WorkspaceLocks for RealLocks {
    fn acquire(&self, root: &Path, user: &str, session: &str, now: u64) -> LockState {
        crate::workspace::acquire_lock(root, user, session, now)
    }
    fn refresh(&self, root: &Path, user: &str, session: &str, now: u64) -> bool {
        crate::workspace::refresh_lock(root, user, session, now)
    }
    fn take_over(&self, root: &Path, user: &str, session: &str, now: u64) {
        crate::workspace::take_over_lock(root, user, session, now)
    }
    fn live(&self, root: &Path, now: u64) -> Option<LockInfo> {
        crate::workspace::lock_live(root, now)
    }
}

pub struct RealDevMode;

impl DevModeFacet for RealDevMode {
    fn is_on(&self, data: &Path) -> bool {
        crate::devmode::is_on(data)
    }
    fn top_sign(&self, data: &Path) -> Option<String> {
        if crate::devmode::update::interrupted(data).is_some() {
            return Some("UPDATE INTERRUPTED \u{00b7} choose \"previous version\" at boot".to_string());
        }
        let d = crate::devmode::DevMode::open(data);
        d.is_on().then(|| match d.shell_banner() {
            Some(b) => format!("DEV \u{00b7} {b}"),
            None if d.pairing().is_some() => "DEV \u{00b7} pairing".to_string(),
            None => "DEV".to_string(),
        })
    }
    fn extend(&self, data: &Path, policy: &Policy, user: &str, hours: u32) -> Result<i64, String> {
        crate::devmode::DevMode::open(data).extend(policy, user, hours).map(|s| s.until)
    }
    fn cert_fingerprint(&self, data: &Path) -> Option<String> {
        crate::devmode::cert_fingerprint(data)
    }
    fn reload_stamp(&self, data: &Path, app: &str) -> u64 {
        crate::devmode::reload_stamp(data, app)
    }
    fn app_log(&self, data: &Path, app: &str, line: &str) {
        crate::devmode::DevMode::open(data).app_log(app, line)
    }
    fn shell_pending(&self, data: &Path) -> Vec<ShellRequest> {
        let d = crate::devmode::DevMode::open(data);
        if d.is_on() { d.shell_pending() } else { Vec::new() }
    }
    fn shell_approve(&self, data: &Path, policy: &Policy, user: &str, computer: &str, minutes: u32) -> Result<(String, String), String> {
        crate::devmode::DevMode::open(data).shell_approve(policy, user, computer, minutes).map(|g| (g.computer, g.kind))
    }
    fn shell_auto_on(&self, data: &Path, policy: &Policy, user: &str, computer: &str, hours: u32) -> Result<(), String> {
        crate::devmode::DevMode::open(data).shell_auto_on(policy, user, computer, hours).map(|_| ())
    }
    fn shell_deny(&self, data: &Path, policy: &Policy, user: &str, computer: &str) -> Result<(), String> {
        crate::devmode::DevMode::open(data).shell_deny(policy, user, computer)
    }
    fn take_open(&self, data: &Path) -> Vec<String> {
        crate::devmode::take_open(data)
    }
    fn set_secret_focus(&self, data: &Path, on: bool) {
        crate::devmode::input::set_secret_focus(data, on)
    }
    fn set_approval_card(&self, data: &Path, on: bool) {
        crate::devmode::input::set_approval_card(data, on)
    }
    fn take_input(&self, data: &Path) -> Vec<InputEv> {
        crate::devmode::input::take(data)
    }
    fn update_interrupted(&self, data: &Path) -> bool {
        crate::devmode::update::interrupted(data).is_some()
    }
    fn update_interrupted_text(&self, data: &Path) -> Option<String> {
        crate::devmode::update::interrupted_text(data)
    }
    fn update_fell_back(&self, data: &Path) -> bool {
        crate::devmode::centre::read_trial(data).is_some_and(|t| t.state == "fell-back")
    }
    fn boot_confirmed(&self, data: &Path) -> Option<String> {
        crate::devmode::DevMode::open(data).restart_started();
        crate::devmode::centre::Centre::real(data).confirm_boot(&crate::devmode::centre::boot_id_now())
    }
    fn compat_lines(&self, data: &Path, admin: bool) -> Vec<String> {
        let dm = crate::devmode::DevMode::open(data);
        let peers = if admin { crate::compatreport::peers(data, &dm.keys(), dm.now()) } else { vec![] };
        crate::compatreport::report(peers).lines(admin)
    }
    fn scrub(&self, line: &str) -> String {
        crate::devmode::debug::scrub(line)
    }
}

impl AgentFacets for AgentApiImpl {
    fn is_real(&self) -> bool {
        true
    }
    fn devmode(&self) -> &dyn DevModeFacet {
        &RealDevMode
    }
    fn exec(&self) -> &dyn ExecFacet {
        &RealExec
    }
    fn workspace_locks(&self) -> &dyn WorkspaceLocks {
        &RealLocks
    }
    fn b0(&self) -> &dyn iyakki_agent_api::B0Facets {
        &crate::facets_b0::RealB0
    }
}
