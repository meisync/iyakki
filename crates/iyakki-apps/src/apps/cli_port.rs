//! What the CLI tile asks of the rest of the system, as one trait (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! The tile (`cli.rs`, `cli_conn.rs`, `cli_term.rs`) names no `Services`, no session, no audit log and no host: it asks a [`CliPort`] who the
//! person is and what the policy allows, for the person's terminal, the saved SSH hosts, the downloaded tools, the Ollama server and the
//! container cluster, the sign-in state of the external coding agents, the power switch and the audit line. The real port ([`CxPort`]) is the
//! only code of the tile that reads the shell's context. Tests drive the tile through the fake port in `cli_fake_tests.rs`.

use std::sync::Arc;

use iyakki_agent_api::{SshHost, Terminal};
use iyakki_sec::policy::CliLevel;
use iyakki_shared::clis::{Installed, Job, SystemStatus};
use iyakki_shared::dlmanifest_fetch::CheckStatus;

use super::AppCx;
use crate::agent_login;

pub trait CliPort {
    // ---- who is asking, and what the policy allows
    /// The signed-in person.
    fn user(&self) -> String;
    /// The words for the person's role ("Administrator", "User" ...).
    fn role_name(&self) -> String;
    /// The person may install, remove and start tools: an administrator with a full terminal.
    fn may_manage_tools(&self) -> bool;
    /// The person's terminal is the full one (ssh, scp and the saved connections need it).
    fn full_terminal(&self) -> bool;
    /// The person's host policy lets them reach `host`.
    fn allows_host(&self, host: &str) -> bool;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);

    // ---- the terminal tab
    /// A terminal for this person: every command goes through their policy.
    fn open_terminal(&self) -> Box<dyn Terminal>;
    /// The `poweroff` command was allowed: switch the computer off.
    fn power_off(&self);
    /// The `reboot` command was allowed: restart the computer.
    fn reboot(&self);
    /// The name of a saved connection the assistant asked to log in to (and the person approved), once.
    fn take_ssh_login(&self) -> Option<String>;

    // ---- the connections tab
    /// The SSH client of this computer for the person: saved connections, trusted host keys, logins.
    fn ssh(&self) -> Arc<dyn SshHost>;
    /// Make a key pair named `name` and keep the private key in the person's vault; the public key comes back.
    fn make_ssh_key(&self, name: &str) -> Result<String, String>;
    /// Is the vault key `name` there? `Err`: the vault is locked.
    fn ssh_key_in_vault(&self, name: &str) -> Result<bool, String>;
    /// Whether an SSH client is on this computer, and how to get one.
    fn ssh_client(&self) -> Option<SystemStatus>;

    // ---- the downloaded tools
    fn tools_installed(&self) -> Vec<Installed>;
    fn tool_installed(&self, id: &str) -> Option<Installed>;
    fn tool_system_status(&self, id: &str) -> Option<SystemStatus>;
    fn tool_job(&self, id: &str) -> Option<Job>;
    /// A download or install is running.
    fn tools_busy(&self) -> bool;
    /// The exact command an install will run, when it runs one.
    fn tool_install_plan(&self, id: &str) -> Option<String>;
    fn tool_install(&self, id: &str) -> Result<(), String>;
    fn tool_remove(&self, id: &str) -> Result<(), String>;
    fn tool_rollback(&self, id: &str) -> Result<String, String>;
    /// Ask for a newer download list now (`force`) or when it is due (a background check).
    fn download_list_check(&self, force: bool);
    /// The audit lines the download-list check made since the last call: (action, detail).
    fn download_list_audit(&self) -> Vec<(String, String)>;
    /// What the last check of the download list says.
    fn download_list_status(&self) -> CheckStatus;

    // ---- the external coding agents (Claude Code, Copilot)
    /// What the AI IDE saw of the agent at the last look.
    fn agent_status(&self, agent: &str) -> Option<iyakki_extagentx::Status>;
    /// The agent has no documented command that reports its sign-in.
    fn agent_sign_in_unchecked(&self, agent: &str) -> bool;
    /// Look at every agent again (a background check).
    fn agent_recheck(&self);
    /// The remembered sign-in state of the agent.
    fn agent_sign_in(&self, agent: &str) -> agent_login::State;

    // ---- the local model server and the container cluster
    fn ollama_state(&self) -> iyakki_shared::ollama::OllamaState;
    fn ollama_url(&self) -> String;
    fn ollama_models_dir(&self) -> std::path::PathBuf;
    fn ollama_start(&self) -> Result<(), String>;
    fn ollama_stop(&self) -> Result<(), String>;
    fn cluster_state(&self) -> iyakki_shared::cluster::ClusterState;
    fn cluster_requirements(&self) -> Vec<String>;
    fn cluster_start(&self) -> Result<(), String>;
    fn cluster_stop(&self) -> Result<(), String>;
}

/// The real port, over the shell's context: the only place the CLI tile touches it.
pub struct CxPort<'a>(pub &'a AppCx);

impl CliPort for CxPort<'_> {
    fn user(&self) -> String {
        self.0.session.user.clone()
    }
    fn role_name(&self) -> String {
        self.0.session.role.name().to_string()
    }
    fn may_manage_tools(&self) -> bool {
        self.0.session.policy.manage_settings && self.0.session.policy.cli == CliLevel::Full
    }
    fn full_terminal(&self) -> bool {
        self.0.session.policy.cli == CliLevel::Full
    }
    fn allows_host(&self, host: &str) -> bool {
        self.0.session.policy.allows_host(host)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }

    fn open_terminal(&self) -> Box<dyn Terminal> {
        self.0.services.terminal(&self.0.session.user, self.0.session.policy.clone(), self.0.host.clone())
    }
    fn power_off(&self) {
        self.0.host.power_off();
    }
    fn reboot(&self) {
        self.0.host.reboot();
    }
    fn take_ssh_login(&self) -> Option<String> {
        self.0.agent.sshsessiontools_take_login(self.0.data_dir(), &self.0.session.user)
    }

    fn ssh(&self) -> Arc<dyn SshHost> {
        self.0.services.ssh_host(&self.0.session.user)
    }
    fn make_ssh_key(&self, name: &str) -> Result<String, String> {
        let (vault, user) = (self.0.vault(), self.0.session.user.clone());
        self.ssh().generate_key(name, &format!("{user}@iyakki"), &move |n, v| vault.user_set(&user, n, v).map_err(|e| format!("could not store the key in your vault: {e}")))
    }
    fn ssh_key_in_vault(&self, name: &str) -> Result<bool, String> {
        self.0.vault().user_names(&self.0.session.user).map(|n| n.iter().any(|k| k == name)).map_err(|e| e.to_string())
    }
    fn ssh_client(&self) -> Option<SystemStatus> {
        self.0.clis().system_status("ssh")
    }

    fn tools_installed(&self) -> Vec<Installed> {
        self.0.clis().installed()
    }
    fn tool_installed(&self, id: &str) -> Option<Installed> {
        self.0.clis().is_installed(id)
    }
    fn tool_system_status(&self, id: &str) -> Option<SystemStatus> {
        self.0.clis().system_status(id)
    }
    fn tool_job(&self, id: &str) -> Option<Job> {
        self.0.clis().job(id)
    }
    fn tools_busy(&self) -> bool {
        self.0.clis().running()
    }
    fn tool_install_plan(&self, id: &str) -> Option<String> {
        self.0.clis().install_plan(id)
    }
    fn tool_install(&self, id: &str) -> Result<(), String> {
        self.0.clis().install(id, &self.0.session.user)
    }
    fn tool_remove(&self, id: &str) -> Result<(), String> {
        self.0.clis().remove(id)
    }
    fn tool_rollback(&self, id: &str) -> Result<String, String> {
        self.0.clis().rollback(id)
    }
    fn download_list_check(&self, force: bool) {
        self.0.agent.helpers_b6().dl_fetch_start(&self.0.clis().manifest_dir(), force);
    }
    fn download_list_audit(&self) -> Vec<(String, String)> {
        self.0.agent.helpers_b6().dl_fetch_take_audit()
    }
    fn download_list_status(&self) -> CheckStatus {
        self.0.agent.helpers_b6().dl_fetch_status(&self.0.clis().manifest_dir())
    }

    fn agent_status(&self, agent: &str) -> Option<iyakki_extagentx::Status> {
        self.0.ext_status().get(agent)
    }
    fn agent_sign_in_unchecked(&self, agent: &str) -> bool {
        self.0.agents_cfg().agent(agent).is_some_and(|a| a.auth_args().is_none())
    }
    fn agent_recheck(&self) {
        self.0.ext_status().refresh(self.0.agents_cfg());
    }
    fn agent_sign_in(&self, agent: &str) -> agent_login::State {
        agent_login::state(self.0, agent, true)
    }

    fn ollama_state(&self) -> iyakki_shared::ollama::OllamaState {
        self.0.ollama().state()
    }
    fn ollama_url(&self) -> String {
        self.0.ollama().url()
    }
    fn ollama_models_dir(&self) -> std::path::PathBuf {
        self.0.ollama().models_dir()
    }
    fn ollama_start(&self) -> Result<(), String> {
        self.0.ollama().start()
    }
    fn ollama_stop(&self) -> Result<(), String> {
        self.0.ollama().stop()
    }
    fn cluster_state(&self) -> iyakki_shared::cluster::ClusterState {
        self.0.sandbox_cluster().state()
    }
    fn cluster_requirements(&self) -> Vec<String> {
        self.0.sandbox_cluster().requirements()
    }
    fn cluster_start(&self) -> Result<(), String> {
        self.0.sandbox_cluster().start()
    }
    fn cluster_stop(&self) -> Result<(), String> {
        self.0.sandbox_cluster().stop()
    }
}
