//! Developer mode as the UI uses it (Settings -> Developer mode, the IDE's deploy): three narrow traits over the agent's `DevMode`, by concern.

use std::path::Path;

use iyakki_sec::policy::Policy;
use iyakki_shared::devmode::grants::ShellRequest;
use iyakki_shared::devmode::scopes::Scope;
use iyakki_shared::devmode_types::grants::{AutoShell, ShellGrant};
use iyakki_shared::devmode_types::pairings::Pairing as PairedComputer;
use iyakki_shared::devmode_types::restartgrant::RestartView;
use iyakki_shared::devmode_types::trusted::{TrustFor, TrustLevel, TrustedView};
use iyakki_shared::devmode_types::{Activity, DevKey, FwWant, Pairing, State};

pub const UNAVAILABLE_DEV: &str = "unavailable: this core does not serve this call";

pub trait DevPairing: Send + Sync {
    fn pairing(&self) -> Option<Pairing> {
        None
    }
    fn start_pairing(&self, _policy: &Policy, _user: &str) -> Result<Pairing, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn cancel_pairing(&self) {}
    fn remove_pairing(&self, _policy: &Policy, _user: &str, _name: &str) -> Result<String, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn paired_computer(&self, _name: &str) -> Option<PairedComputer> {
        None
    }
    fn keys(&self) -> Vec<DevKey> {
        Vec::new()
    }
    fn set_scope(&self, _policy: &Policy, _user: &str, _computer: &str, _s: Scope, _on: bool) -> Result<DevKey, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn set_pairing_scope(&self, _policy: &Policy, _user: &str, _s: Scope, _on: bool) -> Result<Vec<String>, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn grant_diagnostic(&self, _policy: &Policy, _user: &str, _computer: &str) -> Result<DevKey, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn api_scopes(&self, _computer: &str) -> Vec<String> {
        Vec::new()
    }
    fn set_api_scope(&self, _policy: &Policy, _user: &str, _computer: &str, _scope: &str, _on: bool) -> Result<Vec<String>, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn core_scopes(&self, _computer: &str) -> Vec<String> {
        Vec::new()
    }
    fn set_core_scope(&self, _policy: &Policy, _user: &str, _computer: &str, _scope: &str, _on: bool) -> Result<Vec<String>, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn core_enabled(&self) -> bool {
        false
    }
    fn set_core_enabled(&self, _policy: &Policy, _user: &str, _on: bool) -> Result<(), String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn state(&self) -> State {
        State::default()
    }
    fn enable(&self, _policy: &Policy, _user: &str, _hours: u32) -> Result<State, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn extend(&self, _policy: &Policy, _user: &str, _hours: u32) -> Result<State, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn disable(&self, _policy: &Policy, _user: &str) -> Result<(), String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn is_on(&self) -> bool {
        false
    }
    fn now(&self) -> i64 {
        0
    }
    fn status_text(&self) -> String {
        String::new()
    }
    fn activity(&self, _n: usize) -> Vec<Activity> {
        Vec::new()
    }
}

pub trait DevTrust: Send + Sync {
    fn trusted_list(&self) -> Vec<TrustedView> {
        Vec::new()
    }
    fn trusted_on(&self, _policy: &Policy, _user: &str, _computer: &str, _level: TrustLevel, _choice: TrustFor) -> Result<TrustedView, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn trusted_off(&self, _policy: &Policy, _user: &str, _computer: &str) -> Result<(), String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn shell_pending(&self) -> Vec<ShellRequest> {
        Vec::new()
    }
    fn shell_approve(&self, _policy: &Policy, _user: &str, _computer: &str, _minutes: u32) -> Result<ShellGrant, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn shell_deny(&self, _policy: &Policy, _user: &str, _computer: &str) -> Result<(), String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn shell_end(&self, _policy: &Policy, _user: &str, _computer: &str) -> Result<(), String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn shell_auto_list(&self) -> Vec<AutoShell> {
        Vec::new()
    }
    fn shell_auto_on(&self, _policy: &Policy, _user: &str, _computer: &str, _hours: u32) -> Result<AutoShell, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn shell_auto_off(&self, _policy: &Policy, _user: &str, _computer: &str) -> Result<(), String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn restart_grant_list(&self) -> Vec<RestartView> {
        Vec::new()
    }
    fn restart_grant_on(&self, _policy: &Policy, _user: &str, _computer: &str) -> Result<RestartView, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn restart_grant_persist(&self, _policy: &Policy, _user: &str, _computer: &str) -> Result<RestartView, String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn restart_grant_off(&self, _policy: &Policy, _user: &str, _computer: &str) -> Result<(), String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn restart_grant_active(&self, _computer: &str) -> bool {
        false
    }
}

pub trait DevRelay: Send + Sync {
    /// The sessions the paired computers have open now (shell windows, forwarded ports, followed logs).
    fn sessions(&self) -> Vec<iyakki_shared::devmode_types::sessions::SessionInfo> {
        Vec::new()
    }
    /// End every session of a computer; how many were open.
    fn end_sessions_of(&self, _computer: &str) -> usize {
        0
    }
    /// The fingerprint of the certificate the developer port serves with (None: not made yet).
    fn cert_fingerprint(&self) -> Option<String> {
        None
    }
    fn relay_enabled(&self) -> bool {
        false
    }
    fn set_relay_enabled(&self, _policy: &Policy, _user: &str, _on: bool) -> Result<(), String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn relay_url(&self) -> String {
        String::new()
    }
    fn relay_summary(&self) -> String {
        String::new()
    }
    fn network_wide(&self) -> bool {
        false
    }
    fn set_network_wide(&self, _policy: &Policy, _user: &str, _on: bool) -> Result<(), String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn fw_want(&self) -> FwWant {
        FwWant::default()
    }
    fn allow_peer(&self, _policy: &Policy, _user: &str, _addr: &str) -> Result<(), String> {
        Err(UNAVAILABLE_DEV.into())
    }
    fn allow_pairing_through_firewall(&self, _policy: &Policy, _user: &str) -> Result<i64, String> {
        Err(UNAVAILABLE_DEV.into())
    }
}

/// All of developer mode.
pub trait DevModeApi: DevPairing + DevTrust + DevRelay {}
impl<T: DevPairing + DevTrust + DevRelay> DevModeApi for T {}

/// Developer mode of a core that has none: every call answers "unavailable" or nothing.
pub struct NoDev;
impl DevPairing for NoDev {}
impl DevTrust for NoDev {}
impl DevRelay for NoDev {}

/// A developer-mode device an account paired with (`iyakki dev pair`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevDevice {
    pub name: String,
    pub host: String,
    pub port: u16,
}

/// The developer-mode deploy of the IDE: the devices this account paired with, and installing a packed project on this device or on one of them.
pub trait DevDeployFacet: Send + Sync {
    /// The devices this account paired with.
    fn paired_devices(&self, _user: &str) -> Vec<DevDevice> {
        Vec::new()
    }
    /// Pack the project in `dir` (signed with `key`) and install it: on this device (`paired` None) or on the paired device of that name.
    /// Returns (name, kind, summary).
    fn deploy(&self, _user: &str, _policy: &Policy, _paired: Option<&str>, _dir: &Path, _key: &iyakki_shared::store_keys::SigningKey) -> Result<(String, String, String), String> {
        Err(UNAVAILABLE_DEV.into())
    }
}
impl DevDeployFacet for NoDev {}
