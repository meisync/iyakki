//! Hand-written facets of lane q-split-b3 (F-SPLIT-03): what the UI asked of the agent's `profilepolicy`, `browserdev`, ... as narrow traits with
//! "nothing / unavailable" defaults. [`AgentFacetsB3`] is a supertrait of `AgentApi` (the accessor of each facet); the real implementation is
//! `iyakki_agent::facets_b3`, and [`NoB3`] (the defaults) is the fake.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use iyakki_shared::avaddon::Addon;
use iyakki_sec::policy::Policy;
use iyakki_shared::agent::Tools;
use iyakki_shared::clientmedia::{AudioRoutes, Indicator};
use iyakki_shared::hwprofile::Category;
use iyakki_shared::profilepolicy::Beat;
use iyakki_shared::startup::Registry;
use iyakki_shared::vpn_mgr::{Imported as VpnImported, Row as VpnRow, Status as VpnStatus, Summary as VpnSummary};
use iyakki_shared::wizardtools::Backend as WizardBackend;
use iyakki_vpn::profile::{Profile, Settings as VpnSettings};

/// The audit line of a tool wrapper: what happened, with whom.
pub type AuditLine = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// The answer of a call this core does not serve.
pub const UNAVAILABLE_B3: &str = "unavailable: this core does not serve this call";

/// Protected browser data around the screen lock: seal when the screen locks, open again at unlock, the timed write-back.
pub trait ProfileLockFacet: Send + Sync {
    /// The screen locks: seal the person's browser profile when they chose that. `Ok(true)` when it sealed.
    fn on_screen_lock(&self, _data: &Path, _user: &str) -> Result<bool, String> {
        Ok(false)
    }
    /// The password was right: open a profile the lock sealed. `vault_key_hex` is the vault's profile key when the held one is gone.
    fn on_screen_unlock(&self, _data: &Path, _user: &str, _vault_key_hex: Option<&str>) -> Result<bool, String> {
        Ok(false)
    }
    /// One beat of the timed write-back (a thread of its own does the work); `report` hears a written or failed checkpoint.
    fn tick_in_background(&self, _data: &Path, _user: &str, _now: u64, _report: Box<dyn Fn(&Beat) + Send>) {}
}

/// The browser's audio and video add-on: what is installed on this machine (the add-on's texts and the bar words are pure and live in
/// `iyakki_shared::avaddon`).
pub trait AvAddonFacet: Send + Sync {
    /// Is the add-on in the image, installed, complete? Without a core: the image has no bridge, nothing to install can help.
    fn detect(&self, _data: &Path) -> Addon {
        Addon::NotInImage
    }
}

/// Where a person's camera and microphone come from: the web browser they are in, or this machine.
pub trait BrowserSourceFacet: Send + Sync {
    /// Is this person in a web browser session (their media comes from the browser, not from this machine)?
    fn browser_session_active(&self, _user: &str) -> bool {
        false
    }
}

/// Client devices (EXPERIMENTAL, `clientmedia`): what each may do, the red sign and the sound routes.
pub trait ClientMediaFacet: Send + Sync {
    /// Which of the device permissions this device holds now, by capability.
    fn device_holds(&self, _data: &Path, _device_id: &str) -> Vec<String> {
        Vec::new()
    }
    /// Switch one permission of a device on or off; returns the capability changed. The caller writes the audit line.
    fn set_permission(&self, _data: &Path, _device_id: &str, _name: &str, _on: bool, _by: &str, _now: i64) -> Result<&'static str, String> {
        Err(UNAVAILABLE_B3.into())
    }
    /// Which client cameras and microphones this machine is using right now (the red sign).
    fn indicator(&self) -> Indicator {
        Indicator::default()
    }
    /// The routes the mixer's tee feeds (one per process; a core without sound gives an empty set).
    fn audio_routes(&self) -> Arc<AudioRoutes> {
        Arc::new(AudioRoutes::new())
    }
}

/// The assistant's tool wrappers that need no services: each returns the tools with the group put around `inner` (the default returns `inner`).
pub trait ToolWrapFacetB3: Send + Sync {
    /// The client-device tools (administrators decide; the backend holds the data folder).
    fn wrap_device_tools(&self, inner: Arc<dyn Tools>, _data: &Path, _user: &str, _admin: bool, _audit: AuditLine) -> Arc<dyn Tools> {
        inner
    }
    /// The standing restart grant of developer mode: status is free, a change asks every time.
    fn wrap_restart_tools(&self, inner: Arc<dyn Tools>, _data: &Path, _policy: &Policy, _user: &str, _audit: AuditLine) -> Arc<dyn Tools> {
        inner
    }
    /// The setup wizards of installed apps over `backend`.
    fn wrap_wizard_tools(&self, inner: Arc<dyn Tools>, _backend: Arc<dyn WizardBackend>) -> Arc<dyn Tools> {
        inner
    }
}

/// Per-computer settings: the display size of an account follows the computer (a hook of the UI kit calls these).
pub trait ComputerHooksFacet: Send + Sync {
    fn display_size_get(&self, _data: &Path, _user: &str) -> Option<Option<usize>> {
        None
    }
    fn display_size_set(&self, _data: &Path, _user: &str, _level: usize) {}
}

/// What Settings -> This computer shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComputerPage {
    pub me_id: String,
    pub me_label: String,
    /// The categories shared by all computers.
    pub shared: BTreeSet<Category>,
    /// This computer's profile: `(boots, first seen, last seen)` (unix seconds); `None` while it has none.
    pub me_seen: Option<(u32, i64, i64)>,
    /// Every computer this install has seen, this one included: `(id, label, last seen)`.
    pub all: Vec<(String, String, i64)>,
}

impl ComputerPage {
    /// The computers other than this one: `(id, label)`.
    pub fn others(&self) -> Vec<(String, String)> {
        self.all.iter().filter(|(id, _, _)| *id != self.me_id).map(|(id, l, _)| (id.clone(), l.clone())).collect()
    }
}

/// What the shell and the page ask of the agent that needs the services of this install. The real one is built from the services
/// (`Services::b3()` in `iyakki-agent`); [`NoB3`] answers "unavailable" / does nothing.
pub trait ServicesFacetB3: Send + Sync {
    /// The start-up probe: read the machine once and update the registry. `firewall_alert`: the firewall is wanted but not in force.
    fn startup_poll(&self, _reg: &Registry, _firewall_alert: bool) {}
    /// The Retry button of a start-up line; returns the line to show.
    fn startup_retry(&self, _id: &str, _user: &str) -> Result<String, String> {
        Err(UNAVAILABLE_B3.into())
    }
    /// A person signed in: kubeconfigs held in their vaults can be used by cluster commands; `None`: signed out.
    fn kube_resolver(&self, _user: Option<&str>) {}
    /// The real start: recognise this computer, overlay its settings, arm the hooks. Whether a computer was attached.
    fn computer_attach(&self) -> bool {
        false
    }
    /// The one-time card for a computer the install had not seen, to every administrator named; whether a note was raised.
    fn computer_announce(&self, _admins: &[String]) -> bool {
        false
    }
    /// The page of Settings -> This computer (`None`: this install keeps no per-computer settings here).
    fn computer_page(&self) -> Option<ComputerPage> {
        None
    }
    fn computer_set_sharing(&self, _cat: Category, _shared: bool) -> Result<String, String> {
        Err(UNAVAILABLE_B3.into())
    }
    fn computer_copy_from(&self, _name: &str) -> Result<String, String> {
        Err(UNAVAILABLE_B3.into())
    }
    fn computer_reset(&self) -> Result<String, String> {
        Err(UNAVAILABLE_B3.into())
    }
    fn computer_forget(&self, _name: &str) -> Result<String, String> {
        Err(UNAVAILABLE_B3.into())
    }
    /// The real backend of the setup wizards (the person's installed apps, their waiting list and their one run). The default has nothing installed.
    fn wizard_backend(&self, _user: &str, _audit: AuditLine) -> Arc<dyn WizardBackend> {
        Arc::new(NoWizardBackend)
    }
    /// The VPN manager of this person (one shared per person and install).
    fn vpn_manager(&self, _user: &str, _audit: AuditLine) -> Arc<dyn VpnManager> {
        Arc::new(NoB3)
    }
    /// The VPN profiles that were on at the last sign-out start again when the person signs in.
    fn vpn_start_for_login(&self, _user: &str, _audit: AuditLine) {}
    /// A person signed out: their VPN supervision ends.
    fn vpn_on_logout(&self, _user: &str) {}
    /// The VPN tools of the assistant (administrators only: routes are device settings).
    fn wrap_vpn_tools(&self, inner: Arc<dyn Tools>, _user: &str, _audit: AuditLine) -> Arc<dyn Tools> {
        inner
    }
    /// Clone a repository / register an AI IDE workspace as tools (administrators decide per tool); the default returns `inner`.
    fn wrap_git_work_tools(&self, inner: Arc<dyn Tools>, _user: &str, _policy: &Policy, _audit: AuditLine) -> Arc<dyn Tools> {
        inner
    }
}

/// The VPN profiles of one person: what Settings -> Network -> VPN and the doctor ask. The real one is `iyakki_agent::vpn_mgr::Manager`
/// (`ServicesFacetB3::vpn_manager`); the defaults are a person with no profile.
pub trait VpnManager: Send + Sync {
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    fn get(&self, _id: &str) -> Option<Profile> {
        None
    }
    fn is_enabled(&self, _id: &str) -> bool {
        false
    }
    fn rows(&self) -> Vec<VpnRow> {
        Vec::new()
    }
    fn summary(&self) -> Option<VpnSummary> {
        None
    }
    /// The sentence the Firewall page shows while a kill switch or a guard is in force.
    fn guard_line(&self) -> Option<String> {
        None
    }
    fn status(&self, _id: &str) -> VpnStatus {
        VpnStatus::default()
    }
    fn import(&self, _name: &str, _input: &[u8]) -> Result<VpnImported, String> {
        Err(UNAVAILABLE_B3.into())
    }
    fn set_settings(&self, _id: &str, _settings: VpnSettings, _now: u64) -> Result<Profile, String> {
        Err(UNAVAILABLE_B3.into())
    }
    fn set_enabled(&self, _id: &str, _on: bool, _now: u64) -> Result<Profile, String> {
        Err(UNAVAILABLE_B3.into())
    }
    fn remove(&self, _id: &str) -> Result<Vec<String>, String> {
        Err(UNAVAILABLE_B3.into())
    }
    fn retry(&self, _id: &str, _now: u64) {}
    /// Look at the profiles once (the page does it every two seconds while it is open).
    fn tick(&self, _now: u64) {}
    /// Start watching the enabled profiles.
    fn ensure_supervised(&self) {}
}

/// A wizard backend with nothing installed and nothing waiting.
pub struct NoWizardBackend;
impl WizardBackend for NoWizardBackend {
    fn installed(&self) -> Vec<(String, String, iyakki_core_api::wizard::WizardSpec)> {
        Vec::new()
    }
    fn pending(&self) -> Vec<(String, String, String, String)> {
        Vec::new()
    }
    fn clear_pending(&self, _app: &str, _wizard: &str) {}
    fn env(&self) -> Arc<dyn iyakki_shared::wizardrun::Env> {
        Arc::new(NoEnv)
    }
    fn run(&self) -> Arc<std::sync::Mutex<Option<(String, iyakki_shared::wizardrun::Run)>>> {
        Arc::new(std::sync::Mutex::new(None))
    }
    fn audit(&self, _what: &str, _detail: &str) {}
}

struct NoEnv;
impl iyakki_shared::wizardrun::Env for NoEnv {
    fn check(&self, _check: &str, _param: &str) -> Result<(), String> {
        Err(UNAVAILABLE_B3.into())
    }
    fn migrate(&self, _reference: &str, _from: Option<&str>, _to: Option<&str>, _answers: &std::collections::BTreeMap<String, serde_json::Value>) -> Result<String, String> {
        Err(UNAVAILABLE_B3.into())
    }
}

/// The fake for every facet of this file: all defaults.
pub struct NoB3;
impl ProfileLockFacet for NoB3 {}
impl AvAddonFacet for NoB3 {}
impl ClientMediaFacet for NoB3 {}
impl BrowserSourceFacet for NoB3 {}
impl ToolWrapFacetB3 for NoB3 {}
impl ComputerHooksFacet for NoB3 {}
impl ServicesFacetB3 for NoB3 {}
impl VpnManager for NoB3 {}

/// The accessors, one per facet. Every accessor answers with the fake by default.
pub trait AgentFacetsB3: Send + Sync {
    fn profile_lock(&self) -> &dyn ProfileLockFacet {
        &NoB3
    }
    fn avaddon(&self) -> &dyn AvAddonFacet {
        &NoB3
    }
    fn client_media(&self) -> &dyn ClientMediaFacet {
        &NoB3
    }
    fn tool_wraps_b3(&self) -> &dyn ToolWrapFacetB3 {
        &NoB3
    }
    fn browser_source(&self) -> &dyn BrowserSourceFacet {
        &NoB3
    }
    fn computer_hooks(&self) -> &dyn ComputerHooksFacet {
        &NoB3
    }
}

impl AgentFacetsB3 for crate::FakeAgentApi {}
