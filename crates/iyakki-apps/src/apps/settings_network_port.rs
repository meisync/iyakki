//! What the Settings network and sharing pages ask of the rest of the system (lane ST-C of docs/development/SETTINGS-EXTRACTION-PLAN.md):
//! SSH tunnels, VPN, Remote access (+ Connect, Machines, SFTP), Domains, Firewall, Shared folders (+ Share internet), Fleet, Announce and the
//! Network half of the hardware page.
//!
//! [`NetworkSettingsPort`] stands on [`PagePort`] (who is looking, the audit line, the data folder, the clock) and [`UserData`] (the person, their
//! policy, the agent's pure helpers) and adds the managers the pages used to take from `cx.services`, `cx.b3()`, `cx.b4()` and `cx.core`. Each
//! manager comes with the person's audit sink already bound, as before. The real implementation is `impl NetworkSettingsPort for AppCx`; the fake
//! ([`FakeNetworkPort`]) holds in-memory answers a test can set, with the managers' own "unavailable" defaults where it sets nothing.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_agent_api::{DomainsMgr, SftpServer, SharedFolderMgr, SshTunnels, VpnManager};

use crate::apps::chat_port::UserData;
use crate::apps::settings_port::PagePort;
use crate::apps::AppCx;

/// The network pages' view of the shell.
pub(crate) trait NetworkSettingsPort: PagePort + UserData {
    /// The person's SSH tunnels, VPN profiles and domain routes (each writes to the audit log as the person).
    fn ssh_tunnels(&self) -> Arc<dyn SshTunnels>;
    fn vpn_manager(&self) -> Arc<dyn VpnManager>;
    fn domains(&self) -> Arc<dyn DomainsMgr>;
    /// "Serve this desktop to browsers": the switch, the port, and whether an administrator with a password exists to sign in with.
    fn web_access_on(&self) -> bool;
    fn web_access_port(&self) -> u16;
    fn set_web_access(&self, on: bool) -> Result<(), String>;
    fn set_web_access_port(&self, port: u16) -> Result<(), String>;
    fn has_admin_password(&self) -> bool;
    /// Announcing the device name on the local network (mDNS).
    fn announce_on(&self) -> bool;
    fn set_announce(&self, v: Option<bool>) -> Result<(), String>;
    /// The SFTP server of this process (accounts checked against the shell's own), and the first lines of its card.
    fn sftp_server(&self) -> Arc<dyn SftpServer>;
    fn sftp_status_lines(&self) -> Vec<String>;
    /// Fleets: the view, one confirmed act, and how the accept mode reads.
    fn fleet_view(&self) -> Result<iyakki_shared::fleetview::Status, String>;
    fn fleet_act(&self, act: iyakki_shared::fleetview::FleetAct) -> Result<String, String>;
    fn fleet_accept_label(&self, word: &str) -> String;
    /// Shared folders of the person and the person's home folder.
    fn shared_folders(&self) -> Arc<dyn SharedFolderMgr>;
    fn home_root(&self) -> PathBuf;
    /// An audit sink already bound to the person, for a job that outlives the call.
    fn audit_sink(&self) -> Arc<dyn Fn(&str, &str) + Send + Sync>;
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    /// A server with a fake firewall for one test thread (the real one would touch the machine's firewall).
    pub(crate) static TEST_SFTP: std::cell::RefCell<Option<Arc<dyn SftpServer>>> = const { std::cell::RefCell::new(None) };
}

impl NetworkSettingsPort for AppCx {
    fn ssh_tunnels(&self) -> Arc<dyn SshTunnels> {
        let (audit, user) = (self.audit.clone(), self.session.user.clone());
        self.b4().ssh_tunnels(&self.session.user, Arc::new(move |a, d| audit.record(&user, a, d)))
    }
    fn vpn_manager(&self) -> Arc<dyn VpnManager> {
        let (audit, user) = (self.audit.clone(), self.session.user.clone());
        self.b3().vpn_manager(&self.session.user, Arc::new(move |a, d| audit.record(&user, a, d)))
    }
    fn domains(&self) -> Arc<dyn DomainsMgr> {
        let (audit, user) = (self.audit.clone(), self.session.user.clone());
        self.services.domains_facet(&self.session.user, Arc::new(move |a, d| audit.record(&user, a, d)))
    }
    fn web_access_on(&self) -> bool {
        self.services.web_access_on()
    }
    fn web_access_port(&self) -> u16 {
        self.services.web_access_port()
    }
    fn set_web_access(&self, on: bool) -> Result<(), String> {
        self.services.set_web_access(on)
    }
    fn set_web_access_port(&self, port: u16) -> Result<(), String> {
        self.services.set_web_access_port(port)
    }
    fn has_admin_password(&self) -> bool {
        let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
        iyakki_shared::webaccess::has_admin_password(st.list().iter().map(|u| (u.role == iyakki_sec::policy::Role::Admin, !u.hash.is_empty())))
    }
    fn announce_on(&self) -> bool {
        self.services.announce_name_on()
    }
    fn set_announce(&self, v: Option<bool>) -> Result<(), String> {
        self.services.set_announce_name(v)
    }
    fn sftp_server(&self) -> Arc<dyn SftpServer> {
        #[cfg(any(test, feature = "testing"))]
        if let Some(s) = TEST_SFTP.with(|t| t.borrow().clone()) {
            return s;
        }
        let audit = self.audit.clone();
        let (s1, s2) = (self.store.clone(), self.store.clone());
        let login: iyakki_agent_api::SftpLogin = Arc::new(move |n, p| s1.lock().unwrap_or_else(|e| e.into_inner()).authenticate(n, p).map(|s| s.policy).map_err(|e| e.to_string()));
        let lookup: iyakki_agent_api::SftpLookup = Arc::new(move |n| {
            let mut s = s2.lock().unwrap_or_else(|e| e.into_inner());
            s.refresh();
            s.get(n).map(|u| u.policy())
        });
        self.services.sftp_server(self.data_dir(), login, lookup, Arc::new(move |e, d| audit.record("sftp", e, d)))
    }
    fn sftp_status_lines(&self) -> Vec<String> {
        self.services.sftp_status_lines(self.data_dir())
    }
    fn fleet_view(&self) -> Result<iyakki_shared::fleetview::Status, String> {
        let (audit, user) = (self.audit.clone(), self.session.user.clone());
        self.services.fleet_view(self.os.clone(), self.os.fleet(), Arc::new(move |a, d| audit.record(&user, a, d)))
    }
    fn fleet_act(&self, act: iyakki_shared::fleetview::FleetAct) -> Result<String, String> {
        let (audit, user) = (self.audit.clone(), self.session.user.clone());
        self.services.fleet_act(self.os.clone(), self.os.fleet(), Arc::new(move |a, d| audit.record(&user, a, d)), act)
    }
    fn fleet_accept_label(&self, word: &str) -> String {
        self.services.fleet_accept_label(word)
    }
    fn shared_folders(&self) -> Arc<dyn SharedFolderMgr> {
        let (audit, user) = (self.audit.clone(), self.session.user.clone());
        let a2 = user.clone();
        let admin = self.session.role == iyakki_sec::policy::Role::Admin;
        self.agent.host_probe().shared_folders(self.data_dir(), &user, admin, &self.session.policy, Arc::new(move |e, d| audit.record(&a2, e, d)))
    }
    fn home_root(&self) -> PathBuf {
        self.core.file_scope().home_root(&self.session.user, self.data_dir())
    }
    fn audit_sink(&self) -> Arc<dyn Fn(&str, &str) + Send + Sync> {
        let (audit, user) = (self.audit.clone(), self.session.user.clone());
        Arc::new(move |a, d| audit.record(&user, a, d))
    }
}

#[cfg(test)]
pub(crate) use fake::FakeNetworkPort;

#[cfg(test)]
mod fake {
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
    use std::sync::Mutex;

    use iyakki_agent_api::{AgentApi, NoB3, NoB4, NoS6B};
    use iyakki_core_api::Core;
    use iyakki_sec::policy::{Policy, Role};

    use super::*;
    use crate::apps::chat_port::{AgentHelpers, FakeChatPort};

    /// The fake: a signed-in person with a temp data folder, switchable web access, announce and admin-password facts, the managers a test hands
    /// in (the "unavailable" defaults when it hands none), and a record of the fleet acts asked for.
    pub(crate) struct FakeNetworkPort {
        pub chat: FakeChatPort,
        pub tunnels: Mutex<Arc<dyn SshTunnels>>,
        pub vpn: Mutex<Arc<dyn VpnManager>>,
        pub domains: Mutex<Arc<dyn DomainsMgr>>,
        pub folders: Mutex<Arc<dyn SharedFolderMgr>>,
        pub sftp: Mutex<Arc<dyn SftpServer>>,
        pub web_on: AtomicBool,
        pub web_port: AtomicU16,
        /// The next change of the web access or its port answers with this refusal while set.
        pub web_refuses: AtomicBool,
        pub admin_password: AtomicBool,
        pub announce: AtomicBool,
        pub fleet: Mutex<Result<iyakki_shared::fleetview::Status, String>>,
        pub fleet_acts: Mutex<Vec<iyakki_shared::fleetview::FleetAct>>,
        pub fleet_answer: Mutex<Result<String, String>>,
    }

    impl FakeNetworkPort {
        pub(crate) fn new(tag: &str, admin: bool) -> FakeNetworkPort {
            let dir = crate::apps::tests::temp_dir(tag);
            let store = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()));
            let role = if admin { Role::Admin } else { Role::User };
            let agent: Arc<dyn AgentApi> = (crate::testkit::kit().expect("install a test kit").agent)();
            let chat = FakeChatPort::new("ada", role.template(), &dir, Arc::new(iyakki_core_fake::FakeCore::new()) as Arc<dyn Core>, iyakki_agent_api::SettingsRef::of_store(&store)).with_agent(agent);
            FakeNetworkPort {
                chat,
                tunnels: Mutex::new(Arc::new(NoB4)),
                vpn: Mutex::new(Arc::new(NoB3)),
                domains: Mutex::new(Arc::new(NoS6B)),
                folders: Mutex::new(Arc::new(NoS6B)),
                sftp: Mutex::new(Arc::new(iyakki_agent_api::NoSftpServer)),
                web_on: false.into(),
                web_port: 8443.into(),
                web_refuses: false.into(),
                admin_password: true.into(),
                announce: false.into(),
                fleet: Mutex::new(Err("No fleet in this test.".into())),
                fleet_acts: Mutex::new(Vec::new()),
                fleet_answer: Mutex::new(Ok("Done.".into())),
            }
        }
        /// The audit lines as `(action, subject)`.
        pub(crate) fn audited(&self) -> Vec<(String, String)> {
            self.chat.audited().into_iter().map(|(_, a, s)| (a, s)).collect()
        }
        pub(crate) fn dir(&self) -> &Path {
            &self.chat.dir
        }
    }

    impl PagePort for FakeNetworkPort {
        fn is_admin(&self) -> bool {
            self.chat.policy.manage_settings
        }
        fn audit(&self, action: &str, subject: &str) {
            UserData::record_audit(self, action, subject)
        }
        fn data_dir(&self) -> PathBuf {
            self.chat.dir.clone()
        }
        fn now_secs(&self) -> u64 {
            1_700_000_000
        }
    }

    impl AgentHelpers for FakeNetworkPort {
        fn agent(&self) -> Arc<dyn AgentApi> {
            AgentHelpers::agent(&self.chat)
        }
    }

    impl UserData for FakeNetworkPort {
        fn acting_user(&self) -> &str {
            self.chat.acting_user()
        }
        fn data_root(&self) -> &Path {
            self.chat.data_root()
        }
        fn acting_policy(&self) -> &Policy {
            self.chat.acting_policy()
        }
        fn record_audit(&self, action: &str, subject: &str) {
            self.chat.record_audit(action, subject)
        }
        fn core_handle(&self) -> Arc<dyn Core> {
            self.chat.core_handle()
        }
        fn vault_handle(&self) -> Arc<dyn iyakki_agent_api::VaultRead> {
            self.chat.vault_handle()
        }
        fn web_for_user(&self) -> Arc<dyn iyakki_web::Fetch> {
            self.chat.web_for_user()
        }
    }

    impl NetworkSettingsPort for FakeNetworkPort {
        fn ssh_tunnels(&self) -> Arc<dyn SshTunnels> {
            self.tunnels.lock().unwrap().clone()
        }
        fn vpn_manager(&self) -> Arc<dyn VpnManager> {
            self.vpn.lock().unwrap().clone()
        }
        fn domains(&self) -> Arc<dyn DomainsMgr> {
            self.domains.lock().unwrap().clone()
        }
        fn web_access_on(&self) -> bool {
            self.web_on.load(Ordering::SeqCst)
        }
        fn web_access_port(&self) -> u16 {
            self.web_port.load(Ordering::SeqCst)
        }
        fn set_web_access(&self, on: bool) -> Result<(), String> {
            if self.web_refuses.load(Ordering::SeqCst) {
                return Err("The listener could not be started.".into());
            }
            self.web_on.store(on, Ordering::SeqCst);
            Ok(())
        }
        fn set_web_access_port(&self, port: u16) -> Result<(), String> {
            if self.web_refuses.load(Ordering::SeqCst) {
                return Err("The port could not be changed.".into());
            }
            self.web_port.store(port, Ordering::SeqCst);
            Ok(())
        }
        fn has_admin_password(&self) -> bool {
            self.admin_password.load(Ordering::SeqCst)
        }
        fn announce_on(&self) -> bool {
            self.announce.load(Ordering::SeqCst)
        }
        fn set_announce(&self, v: Option<bool>) -> Result<(), String> {
            self.announce.store(v.unwrap_or(false), Ordering::SeqCst);
            Ok(())
        }
        fn sftp_server(&self) -> Arc<dyn SftpServer> {
            self.sftp.lock().unwrap().clone()
        }
        fn sftp_status_lines(&self) -> Vec<String> {
            vec!["SFTP is off.".into()]
        }
        fn fleet_view(&self) -> Result<iyakki_shared::fleetview::Status, String> {
            self.fleet.lock().unwrap().clone()
        }
        fn fleet_act(&self, act: iyakki_shared::fleetview::FleetAct) -> Result<String, String> {
            self.fleet_acts.lock().unwrap().push(act);
            self.fleet_answer.lock().unwrap().clone()
        }
        fn fleet_accept_label(&self, word: &str) -> String {
            word.to_string()
        }
        fn shared_folders(&self) -> Arc<dyn SharedFolderMgr> {
            self.folders.lock().unwrap().clone()
        }
        fn home_root(&self) -> PathBuf {
            self.chat.dir.join("home")
        }
        fn audit_sink(&self) -> Arc<dyn Fn(&str, &str) + Send + Sync> {
            crate::apps::chat_port::ChatPort::audit_sink(&self.chat)
        }
    }
}
