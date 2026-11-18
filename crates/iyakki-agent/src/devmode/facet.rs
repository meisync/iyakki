//! The real developer mode behind `iyakki_agent_api::facets_dev` (F-SPLIT-03).

use std::sync::Arc;

use iyakki_agent_api::{DevModeApi, DevPairing, DevRelay, DevTrust};
use iyakki_sec::policy::Policy;
use iyakki_shared::devmode::grants::ShellRequest;
use iyakki_shared::devmode::scopes::Scope;
use iyakki_shared::devmode_types::grants::{AutoShell, ShellGrant};
use iyakki_shared::devmode_types::pairings::Pairing as PairedComputer;
use iyakki_shared::devmode_types::restartgrant::RestartView;
use iyakki_shared::devmode_types::trusted::{TrustFor, TrustLevel, TrustedView};
use iyakki_shared::devmode_types::{Activity, DevKey, FwWant, Pairing, State};

use super::DevMode;

/// A developer mode over a data folder, as the UI's handle.
pub fn open(data: &std::path::Path) -> Arc<dyn DevModeApi> {
    Arc::new(DevMode::open(data))
}

impl DevPairing for DevMode {
    fn pairing(&self) -> Option<Pairing> {
        DevMode::pairing(self)
    }
    fn start_pairing(&self, policy: &Policy, user: &str) -> Result<Pairing, String> {
        DevMode::start_pairing(self, policy, user)
    }
    fn cancel_pairing(&self) {
        DevMode::cancel_pairing(self)
    }
    fn remove_pairing(&self, policy: &Policy, user: &str, name: &str) -> Result<String, String> {
        DevMode::remove_pairing(self, policy, user, name)
    }
    fn paired_computer(&self, name: &str) -> Option<PairedComputer> {
        DevMode::paired_computer(self, name)
    }
    fn keys(&self) -> Vec<DevKey> {
        DevMode::keys(self)
    }
    fn set_scope(&self, policy: &Policy, user: &str, computer: &str, s: Scope, on: bool) -> Result<DevKey, String> {
        DevMode::set_scope(self, policy, user, computer, s, on)
    }
    fn set_pairing_scope(&self, policy: &Policy, user: &str, s: Scope, on: bool) -> Result<Vec<String>, String> {
        DevMode::set_pairing_scope(self, policy, user, s, on)
    }
    fn grant_diagnostic(&self, policy: &Policy, user: &str, computer: &str) -> Result<DevKey, String> {
        DevMode::grant_diagnostic(self, policy, user, computer)
    }
    fn api_scopes(&self, computer: &str) -> Vec<String> {
        DevMode::api_scopes(self, computer)
    }
    fn set_api_scope(&self, policy: &Policy, user: &str, computer: &str, scope: &str, on: bool) -> Result<Vec<String>, String> {
        DevMode::set_api_scope(self, policy, user, computer, scope, on)
    }
    fn core_scopes(&self, computer: &str) -> Vec<String> {
        DevMode::core_scopes(self, computer)
    }
    fn set_core_scope(&self, policy: &Policy, user: &str, computer: &str, scope: &str, on: bool) -> Result<Vec<String>, String> {
        DevMode::set_core_scope(self, policy, user, computer, scope, on)
    }
    fn core_enabled(&self) -> bool {
        DevMode::core_enabled(self)
    }
    fn set_core_enabled(&self, policy: &Policy, user: &str, on: bool) -> Result<(), String> {
        DevMode::set_core_enabled(self, policy, user, on)
    }
    fn state(&self) -> State {
        DevMode::state(self)
    }
    fn enable(&self, policy: &Policy, user: &str, hours: u32) -> Result<State, String> {
        DevMode::enable(self, policy, user, hours)
    }
    fn extend(&self, policy: &Policy, user: &str, hours: u32) -> Result<State, String> {
        DevMode::extend(self, policy, user, hours)
    }
    fn disable(&self, policy: &Policy, user: &str) -> Result<(), String> {
        DevMode::disable(self, policy, user)
    }
    fn is_on(&self) -> bool {
        DevMode::is_on(self)
    }
    fn now(&self) -> i64 {
        DevMode::now(self)
    }
    fn status_text(&self) -> String {
        DevMode::status_text(self)
    }
    fn activity(&self, n: usize) -> Vec<Activity> {
        DevMode::activity(self, n)
    }
}

impl DevTrust for DevMode {
    fn trusted_list(&self) -> Vec<TrustedView> {
        DevMode::trusted_list(self)
    }
    fn trusted_on(&self, policy: &Policy, user: &str, computer: &str, level: TrustLevel, choice: TrustFor) -> Result<TrustedView, String> {
        DevMode::trusted_on(self, policy, user, computer, level, choice)
    }
    fn trusted_off(&self, policy: &Policy, user: &str, computer: &str) -> Result<(), String> {
        DevMode::trusted_off(self, policy, user, computer)
    }
    fn shell_pending(&self) -> Vec<ShellRequest> {
        DevMode::shell_pending(self)
    }
    fn shell_approve(&self, policy: &Policy, user: &str, computer: &str, minutes: u32) -> Result<ShellGrant, String> {
        DevMode::shell_approve(self, policy, user, computer, minutes)
    }
    fn shell_deny(&self, policy: &Policy, user: &str, computer: &str) -> Result<(), String> {
        DevMode::shell_deny(self, policy, user, computer)
    }
    fn shell_end(&self, policy: &Policy, user: &str, computer: &str) -> Result<(), String> {
        DevMode::shell_end(self, policy, user, computer)
    }
    fn shell_auto_list(&self) -> Vec<AutoShell> {
        DevMode::shell_auto_list(self)
    }
    fn shell_auto_on(&self, policy: &Policy, user: &str, computer: &str, hours: u32) -> Result<AutoShell, String> {
        DevMode::shell_auto_on(self, policy, user, computer, hours)
    }
    fn shell_auto_off(&self, policy: &Policy, user: &str, computer: &str) -> Result<(), String> {
        DevMode::shell_auto_off(self, policy, user, computer)
    }
    fn restart_grant_list(&self) -> Vec<RestartView> {
        DevMode::restart_grant_list(self)
    }
    fn restart_grant_on(&self, policy: &Policy, user: &str, computer: &str) -> Result<RestartView, String> {
        DevMode::restart_grant_on(self, policy, user, computer)
    }
    fn restart_grant_persist(&self, policy: &Policy, user: &str, computer: &str) -> Result<RestartView, String> {
        DevMode::restart_grant_persist(self, policy, user, computer)
    }
    fn restart_grant_off(&self, policy: &Policy, user: &str, computer: &str) -> Result<(), String> {
        DevMode::restart_grant_off(self, policy, user, computer)
    }
    fn restart_grant_active(&self, computer: &str) -> bool {
        DevMode::restart_grant_active(self, computer)
    }
}

impl DevRelay for DevMode {
    fn sessions(&self) -> Vec<iyakki_shared::devmode_types::sessions::SessionInfo> {
        super::sessions::list(self.data_dir())
    }
    fn end_sessions_of(&self, computer: &str) -> usize {
        super::sessions::end_computer(self.data_dir(), computer)
    }
    fn cert_fingerprint(&self) -> Option<String> {
        super::cert_fingerprint(self.data_dir())
    }
    fn relay_enabled(&self) -> bool {
        DevMode::relay_enabled(self)
    }
    fn set_relay_enabled(&self, policy: &Policy, user: &str, on: bool) -> Result<(), String> {
        DevMode::set_relay_enabled(self, policy, user, on)
    }
    fn relay_url(&self) -> String {
        DevMode::relay_url(self)
    }
    fn relay_summary(&self) -> String {
        DevMode::relay_summary(self)
    }
    fn network_wide(&self) -> bool {
        DevMode::network_wide(self)
    }
    fn set_network_wide(&self, policy: &Policy, user: &str, on: bool) -> Result<(), String> {
        DevMode::set_network_wide(self, policy, user, on)
    }
    fn fw_want(&self) -> FwWant {
        DevMode::fw_want(self)
    }
    fn allow_peer(&self, policy: &Policy, user: &str, addr: &str) -> Result<(), String> {
        DevMode::allow_peer(self, policy, user, addr)
    }
    fn allow_pairing_through_firewall(&self, policy: &Policy, user: &str) -> Result<i64, String> {
        DevMode::allow_pairing_through_firewall(self, policy, user)
    }
}


impl crate::services::Services {
    /// The developer-mode deploy of the IDE over these services (`cx.services.dev_deploy()`).
    pub fn dev_deploy(self: &Arc<Self>) -> Arc<dyn iyakki_agent_api::DevDeployFacet> {
        Arc::new(RealDeploy { svc: self.clone() })
    }
}

struct RealDeploy {
    svc: Arc<crate::services::Services>,
}

impl RealDeploy {
    fn home(&self, user: &str) -> super::client::Home {
        super::client::Home::at(&self.svc.dir().join("devmode").join("client").join(user))
    }
}

impl iyakki_agent_api::DevDeployFacet for RealDeploy {
    fn paired_devices(&self, user: &str) -> Vec<iyakki_agent_api::DevDevice> {
        self.home(user).devices().into_iter().map(|d| iyakki_agent_api::DevDevice { name: d.name, host: d.host, port: d.port }).collect()
    }

    fn deploy(&self, user: &str, policy: &Policy, paired: Option<&str>, dir: &std::path::Path, key: &iyakki_shared::store_keys::SigningKey) -> Result<(String, String, String), String> {
        use iyakki_shared::store_install::Actor;
        let bytes = super::ops::package_bytes(dir, key)?;
        match paired {
            None => {
                let d = DevMode::open(self.svc.dir());
                if !d.is_on() {
                    return Err("Developer mode is off on this device: an administrator switches it on in Settings, Developer mode.".into());
                }
                let store = crate::store::Store::for_services(&self.svc);
                let actor = Actor::from_policy(user, policy);
                let who = super::ops::Who { computer: "this-device-ide".into(), account: user.to_string() };
                let r = super::ops::install(&self.svc, &store, &d, &actor, &who, bytes, false)?;
                Ok((r.name.clone(), r.kind.clone(), format!("{}:{} {} installed on this device for {}", r.kind, r.name, r.version, r.scope.trim_start_matches("user/"))))
            }
            Some(name) => {
                let home = self.home(user);
                let dev = home.devices().into_iter().find(|d| d.name == name).ok_or("That device is no longer paired.")?;
                let c = super::client::DevClient::new(&home, dev.clone())?;
                let v = c.install(&bytes, false)?;
                let (name, kind) = (v["name"].as_str().unwrap_or("").to_string(), v["kind"].as_str().unwrap_or("").to_string());
                if kind == "tile" {
                    let _ = c.open(&name);
                }
                Ok((name.clone(), kind.clone(), format!("{kind}:{name} {} installed on {} ({}:{})", v["version"].as_str().unwrap_or(""), dev.name, dev.host, dev.port)))
            }
        }
    }
}
