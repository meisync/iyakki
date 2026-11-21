//! What the LINUX WINDOW tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`linuxwin.rs`) names no `Services`, no session and no audit log: it asks a [`LinuxWinPort`] for who is signed in and what the
//! person's policy allows, whether the base-system install job is running, and to start that install. The sandbox manager, the spawner and the
//! process table stay the tile's `Rig` (tests replace it); the real rig comes from the port. The real port ([`CxPort`]) is the only place that
//! reads the session, the jobs and the quick-install service and writes the audit line.

use iyakki_sec::policy::CliLevel;
use iyakki_shared::linuxapps::LinuxApps;
use iyakki_shared::linuxwin as lw;
use iyakki_shared::quickinstall as qi;
use std::sync::Arc;

use super::linuxwin::Rig;
use super::AppCx;

pub trait LinuxWinPort {
    /// The signed-in person's name.
    fn user(&self) -> String;
    /// The person may install programs for the sandbox (administrators).
    fn can_install(&self) -> bool;
    /// The person has the full command-line permission (Linux windows run programs of the person's choosing).
    fn full_cli(&self) -> bool;
    /// The person's network policy is "any host".
    fn all_hosts(&self) -> bool;
    /// The "Install Linux windows" job of this person is running now.
    fn base_install_running(&self) -> bool;
    /// Start that install as a detached job (administrators); the audit line is written here.
    fn start_base_install(&self) -> Result<(), String>;
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl CxPort<'_> {
    /// The real rig: the sandbox manager of the signed-in person, the real spawner and this machine's process table.
    pub fn rig(&self) -> Rig {
        let data = self.0.data_dir().to_path_buf();
        let user = self.0.session.user.clone();
        Rig {
            la: Arc::new(move || {
                let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
                Ok(LinuxApps::new(&data, &user, &exe, crate::agent().exec().system()))
            }),
            spawn: Arc::new(lw::System),
            proc_root: self.0.core.host().proc_root(),
        }
    }
}

impl LinuxWinPort for CxPort<'_> {
    fn user(&self) -> String {
        self.0.session.user.clone()
    }
    fn can_install(&self) -> bool {
        self.0.session.policy.manage_settings
    }
    fn full_cli(&self) -> bool {
        self.0.session.policy.cli == CliLevel::Full
    }
    fn all_hosts(&self) -> bool {
        self.0.session.policy.net_hosts.iter().any(|h| h == "*")
    }
    fn base_install_running(&self) -> bool {
        self.0.jobs().visible(&self.0.session.user, qi::WORKSPACE).iter().any(|j| j.running() && qi::item_of(j) == Some(qi::Item::LinuxWindows))
    }
    fn start_base_install(&self) -> Result<(), String> {
        let l = qi::Launch::this_program(self.0.data_dir())?;
        self.0.b0().qi_start(&self.0.session.user, qi::Item::LinuxWindows, &|_| false, &l)?;
        self.0.audit.record(&self.0.session.user, "quickinstall.start", "linux-windows");
        Ok(())
    }
}
