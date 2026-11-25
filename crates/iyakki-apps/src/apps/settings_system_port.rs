//! What the system pages of Settings (lane ST-A: Startup, Cloud, Overview, Time, Memory, Install, Computer, Power, Session log, Updates, ...)
//! ask of the rest of the system. One small trait per page; the real implementation is `impl X for AppCx` here, the one place that reads the
//! session, the services and the core for those pages. Each page names its own trait (a `p` parameter, never `cx`); the fake is
//! [`FakeSys`], with an administrator switch, an audit list, a manual clock and in-memory state for each page.
//! docs/development/SETTINGS-EXTRACTION-PLAN.md section 2.

use super::super::settings_port::PagePort;
use super::super::AppCx;
use iyakki_shared::startup::{self, Row, Summary};

// ---------------------------------------------------------------------------------------------------------------- Startup

/// The startup registry and its Retry.
pub(crate) trait StartupPort: PagePort {
    /// Every service with where its start stands.
    fn startup_rows(&self) -> Vec<Row>;
    /// The one-line summary of the registry.
    fn startup_summary(&self) -> Summary;
    /// Look at the services again now.
    fn startup_poll(&self);
    /// Start the service again (in the background); the audit line is the caller's.
    fn startup_retry(&self, id: &'static str);
}

impl StartupPort for AppCx {
    fn startup_rows(&self) -> Vec<Row> {
        startup::global().snapshot()
    }
    fn startup_summary(&self) -> Summary {
        startup::global().summary()
    }
    fn startup_poll(&self) {
        self.b3().startup_poll(startup::global(), iyakki_fw::service::alert());
    }
    fn startup_retry(&self, id: &'static str) {
        let (svc, user) = (self.services.clone(), self.session.user.clone());
        std::thread::spawn(move || {
            let _ = svc.b3().startup_retry(id, &user);
        });
    }
}

// ----------------------------------------------------------------------------------------------------------------- Cloud

/// The Cloud mode page: the mode setting, "show all AI tools" and what this machine is.
pub(crate) trait CloudPort: PagePort {
    fn cloud_setting(&self) -> iyakki_cloudmode::Setting;
    fn set_cloud_setting(&self, v: iyakki_cloudmode::Setting) -> Result<(), String>;
    fn show_all_ai(&self) -> bool;
    fn set_show_all_ai(&self, v: bool) -> Result<(), String>;
    /// One line about this machine's AI hardware.
    fn ai_machine_summary(&self) -> String;
}

impl CloudPort for AppCx {
    fn cloud_setting(&self) -> iyakki_cloudmode::Setting {
        self.services.cloud_setting()
    }
    fn set_cloud_setting(&self, v: iyakki_cloudmode::Setting) -> Result<(), String> {
        self.services.set_cloud_setting(v)
    }
    fn show_all_ai(&self) -> bool {
        self.services.show_all_ai()
    }
    fn set_show_all_ai(&self, v: bool) -> Result<(), String> {
        self.services.set_show_all_ai(v)
    }
    fn ai_machine_summary(&self) -> String {
        self.agent.ai_profile().machine(self.data_dir()).summary()
    }
}

// -------------------------------------------------------------------------------------------------- Session log (crash log)

/// The kept kernel logs and the person's home folder (where a copy goes).
pub(crate) trait CrashPort: PagePort {
    fn crashlog(&self) -> std::sync::Arc<dyn iyakki_core_api::crashlog::CrashLog>;
    /// The folder a copy of the log is written to (the person's home).
    fn home_for_writes(&self) -> std::path::PathBuf;
}

impl CrashPort for AppCx {
    fn crashlog(&self) -> std::sync::Arc<dyn iyakki_core_api::crashlog::CrashLog> {
        self.os.crashlog()
    }
    fn home_for_writes(&self) -> std::path::PathBuf {
        self.core.file_scope().home_root_for_writes(&self.session.user, self.data_dir())
    }
}

// --------------------------------------------------------------------------------------------------------------- Overview

/// What the Overview page asks about the rest of the system to say what needs attention.
pub(crate) trait OverviewPort: CrashPort {
    /// An AI model is chosen for the assistant.
    fn chat_chosen(&self) -> bool;
    /// How many setup wizards wait for this person.
    fn pending_wizard_count(&self) -> usize;
    /// The words about a system update that did not finish.
    fn update_interrupted_text(&self, data: &std::path::Path) -> Option<String>;
    /// The last update did not start and the saved version runs.
    fn update_fell_back(&self, data: &std::path::Path) -> bool;
    /// The words of the "updates available" badge, when the last check found some.
    fn update_badge(&self) -> Option<String>;
    /// The data folder lives in memory only.
    fn data_in_memory(&self, dir: &std::path::Path) -> bool;
    /// This account may open the DOCTOR app (Share diagnostics).
    fn allows_doctor(&self) -> bool;
}

impl OverviewPort for AppCx {
    fn chat_chosen(&self) -> bool {
        self.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get().chat.is_some()
    }
    fn pending_wizard_count(&self) -> usize {
        self.storeapi().pending_wizards(&self.session.user).len()
    }
    fn update_interrupted_text(&self, data: &std::path::Path) -> Option<String> {
        self.agent.devmode().update_interrupted_text(data)
    }
    fn update_fell_back(&self, data: &std::path::Path) -> bool {
        self.agent.devmode().update_fell_back(data)
    }
    fn update_badge(&self) -> Option<String> {
        self.storeapi().update_report().and_then(|r| r.badge())
    }
    fn data_in_memory(&self, dir: &std::path::Path) -> bool {
        self.os.memory().data_place(dir) == iyakki_core_api::memory::DataPlace::Memory
    }
    fn allows_doctor(&self) -> bool {
        self.session.policy.allows_app(iyakki_sec::policy::App::Doctor)
    }
}

// ------------------------------------------------------------------------------------------------ Notifications and Matrix

/// The Matrix account (where to reach me) and the network policy for its server.
pub(crate) trait MatrixPort: PagePort {
    /// The person's Matrix connector (its own changes are audited by the service through the person's log).
    fn matrix(&self) -> std::sync::Arc<dyn iyakki_agent_api::MatrixService>;
    /// Check a homeserver address and give it in its clean form.
    fn matrix_check_homeserver(&self, addr: &str) -> Result<String, String>;
    /// The host name inside an address.
    fn host_of(&self, url: &str) -> String;
    /// The person's network policy lets them reach `host`.
    fn allows_host(&self, host: &str) -> bool;
}

impl MatrixPort for AppCx {
    fn matrix(&self) -> std::sync::Arc<dyn iyakki_agent_api::MatrixService> {
        let audit = self.audit.clone();
        let user = self.session.user.clone();
        let log: iyakki_shared::gitkeys::AuditFn = std::sync::Arc::new(move |a, d| audit.record(&user, a, d));
        self.services.matrix(&self.session.user, log)
    }
    fn matrix_check_homeserver(&self, addr: &str) -> Result<String, String> {
        self.agent.matrix_check_homeserver(addr)
    }
    fn host_of(&self, url: &str) -> String {
        self.agent.gitconn_host_of(url)
    }
    fn allows_host(&self, host: &str) -> bool {
        self.session.policy.allows_host(host)
    }
}

/// The notification levels of the person's inbox and the camera description switch.
pub(crate) trait NotifsPort: MatrixPort {
    /// (source, label) of everything that leaves notes in the inbox.
    fn notif_sources(&self) -> Vec<(String, String)>;
    fn notif_levels(&self) -> std::collections::BTreeMap<String, iyakki_shared::inbox::Level>;
    fn notif_level(&self, source: &str) -> iyakki_shared::inbox::Level;
    fn notif_set_level(&self, source: &str, level: iyakki_shared::inbox::Level) -> Result<(), String>;
    /// Camera alerts are described by the chat model.
    fn camvision_on(&self) -> bool;
    fn camvision_set(&self, on: bool) -> Result<(), String>;
}

impl NotifsPort for AppCx {
    fn notif_sources(&self) -> Vec<(String, String)> {
        self.inbox().sources(&self.session.user)
    }
    fn notif_levels(&self) -> std::collections::BTreeMap<String, iyakki_shared::inbox::Level> {
        self.inbox().levels(&self.session.user)
    }
    fn notif_level(&self, source: &str) -> iyakki_shared::inbox::Level {
        self.inbox().level(&self.session.user, source)
    }
    fn notif_set_level(&self, source: &str, level: iyakki_shared::inbox::Level) -> Result<(), String> {
        self.inbox().set_level(&self.session.user, source, level)
    }
    fn camvision_on(&self) -> bool {
        iyakki_shared::camvision::Setting::load(self.data_dir(), &self.session.user).on
    }
    fn camvision_set(&self, on: bool) -> Result<(), String> {
        (iyakki_shared::camvision::Setting { on }).save(self.data_dir(), &self.session.user)
    }
}

// ---------------------------------------------------------------------------------------------------------------- Storage

/// Whether this account may restart or power off the machine (Storage and Install offer a restart).
pub(crate) trait RestartPort: PagePort {
    fn allows_power(&self) -> bool;
}

impl RestartPort for AppCx {
    fn allows_power(&self) -> bool {
        self.session.policy.power
    }
}

/// The Storage page: the drive the system runs from, the person's drives, and who may open DISKS or restart.
pub(crate) trait StoragePort: RestartPort {
    /// This account may open the DISKS app.
    fn allows_disks(&self) -> bool;
    /// The person's home folder as text (where the drives open).
    fn home_root(&self, data_dir: &std::path::Path) -> String;
    /// The folder the person's drives open under.
    fn disks_media_dir(&self, home: &str) -> String;
    /// Drives are closed when the person signs out.
    fn disks_signout_pref(&self, home: &str) -> bool;
    fn disks_set_signout_pref(&self, home: &str, on: bool) -> Result<(), String>;
    /// How the boot drive is connected and whether it is healthy.
    fn usb_loss_snapshot(&self) -> iyakki_core_api::device_types::storage::loss::Snapshot;
    /// Switch the compatible USB mode (no UAS) for the next start; the plain words of the result.
    fn set_usb_compat(&self, on: bool, usb_id: &str) -> Result<String, String>;
}

impl StoragePort for AppCx {
    fn allows_disks(&self) -> bool {
        self.session.policy.allows_app(iyakki_sec::policy::App::Disks)
    }
    fn home_root(&self, data_dir: &std::path::Path) -> String {
        self.core.file_scope().home_root(&self.session.user, data_dir).display().to_string()
    }
    fn disks_media_dir(&self, home: &str) -> String {
        self.agent.disks_media_dir(home)
    }
    fn disks_signout_pref(&self, home: &str) -> bool {
        self.agent.disks_signout_pref(home)
    }
    fn disks_set_signout_pref(&self, home: &str, on: bool) -> Result<(), String> {
        self.agent.disks_set_signout_pref(home, on)
    }
    fn usb_loss_snapshot(&self) -> iyakki_core_api::device_types::storage::loss::Snapshot {
        iyakki_core_api::device_types::storage::loss::snapshot()
    }
    fn set_usb_compat(&self, on: bool, usb_id: &str) -> Result<String, String> {
        iyakki_core_api::device_types::storage::loss::set_compat(on, usb_id)
    }
}

// ------------------------------------------------------------------------------------------------------------ Time, Host name

/// The Time page: the machine's time settings, the person's clock face, network time and the zone lookup.
pub(crate) trait TimePort: PagePort {
    fn time_settings(&self) -> iyakki_shared::settings::TimeSettings;
    /// Change the time settings with `f` (checked and saved like any other change).
    fn time_update(&self, f: &mut dyn FnMut(&mut iyakki_shared::settings::TimeSettings)) -> Result<(), String>;
    /// One setting through the core contract (`settings.get`); `None` when refused.
    fn contract_setting(&self, key: &str) -> Option<iyakki_core_api::contract::SettingValue>;
    /// The person's own clock face: (12-hour, seconds).
    fn clock_face(&self) -> (bool, bool);
    fn set_clock_face(&self, hour12: bool, seconds: bool);
    /// The servers to ask for the time, the person's own first.
    fn ntp_servers_for(&self, custom: &str) -> Vec<String>;
    fn ntp_valid_server(&self, s: &str) -> bool;
    fn ntp_status(&self) -> iyakki_shared::ntp::Status;
    /// A job that asks the servers once and sets the clock with `setter`; the answer is its status words.
    fn ntp_sync_job(&self, servers: Vec<String>, allow_back: bool, setter: super::time::ClockSetter) -> Box<dyn FnOnce() -> String + Send>;
    /// Look the zone up in the background; `false` when a lookup already runs.
    fn zone_lookup_in_background(&self, on_change: std::sync::Arc<dyn Fn(&str) + Send + Sync>) -> bool;
}

impl TimePort for AppCx {
    fn time_settings(&self) -> iyakki_shared::settings::TimeSettings {
        self.settings().lock().map(|s| s.get().time.clone()).unwrap_or_default()
    }
    fn time_update(&self, f: &mut dyn FnMut(&mut iyakki_shared::settings::TimeSettings)) -> Result<(), String> {
        self.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).update(|s| f(&mut s.time)).map_err(|e| e.to_string())
    }
    fn contract_setting(&self, key: &str) -> Option<iyakki_core_api::contract::SettingValue> {
        use iyakki_core_api::contract::CallContext;
        let ctx = CallContext::local(&self.session.user, self.session.policy.manage_settings);
        self.services.contract_local().settings().and_then(|s| s.get(&ctx, key).ok())
    }
    fn clock_face(&self) -> (bool, bool) {
        crate::uisize::load_clock(self.data_dir(), &self.session.user)
    }
    fn set_clock_face(&self, hour12: bool, seconds: bool) {
        crate::uisize::save_clock(self.data_dir(), &self.session.user, hour12, seconds);
    }
    fn ntp_servers_for(&self, custom: &str) -> Vec<String> {
        self.agent.ntp_servers_for(custom)
    }
    fn ntp_valid_server(&self, s: &str) -> bool {
        self.agent.ntp_valid_server(s)
    }
    fn ntp_status(&self) -> iyakki_shared::ntp::Status {
        self.agent.ntp().status()
    }
    fn ntp_sync_job(&self, servers: Vec<String>, allow_back: bool, setter: super::time::ClockSetter) -> Box<dyn FnOnce() -> String + Send> {
        let agent = self.agent.clone();
        Box::new(move || agent.ntp().sync_once(&servers, allow_back, std::time::Duration::from_secs(4), &*setter).message)
    }
    fn zone_lookup_in_background(&self, on_change: std::sync::Arc<dyn Fn(&str) + Send + Sync>) -> bool {
        self.b0().tz_refresh_in_background(true, on_change)
    }
}

/// The Host name page: where the name is kept and how it is applied.
pub(crate) trait HostNamePort: PagePort {
    /// The file that keeps the name for the next boots (`system.conf`).
    fn hostname_conf_path(&self) -> std::path::PathBuf;
    /// Apply the name to the running system (the core asks first).
    fn set_hostname(&self, name: &str, etc_hostname: &std::path::Path) -> Result<(), String>;
}

impl HostNamePort for AppCx {
    fn hostname_conf_path(&self) -> std::path::PathBuf {
        self.os.memory().conf_path()
    }
    fn set_hostname(&self, name: &str, etc_hostname: &std::path::Path) -> Result<(), String> {
        self.os.hardware().set_hostname(name, etc_hostname)
    }
}

// ------------------------------------------------------------------------------------------------------------------- Power

/// The Power page: the choices live in the settings file; the processor, sleep and fan are read and changed through the host probe.
pub(crate) trait PowerPort: super::super::settings_port::SettingsDataPort {
    fn power_caps(&self, env: &iyakki_shared::power::PowerEnv) -> iyakki_shared::power::Caps;
    fn power_current(&self, env: &iyakki_shared::power::PowerEnv) -> Option<iyakki_shared::power::Profile>;
    /// Write the profile to the processor; how many policies took it.
    fn power_set_profile(&self, env: &iyakki_shared::power::PowerEnv, p: iyakki_shared::power::Profile) -> Result<usize, String>;
    /// The fan card, only on a computer with a fan the system can run.
    fn power_fan_card(&self, env: &iyakki_shared::power::PowerEnv) -> Option<iyakki_shared::power::FanCard>;
    /// Switch fan control; the new state.
    fn power_fan_toggle(&self, env: &iyakki_shared::power::PowerEnv) -> Result<bool, String>;
}

impl PowerPort for AppCx {
    fn power_caps(&self, env: &iyakki_shared::power::PowerEnv) -> iyakki_shared::power::Caps {
        crate::agent().host_probe().power_caps(env)
    }
    fn power_current(&self, env: &iyakki_shared::power::PowerEnv) -> Option<iyakki_shared::power::Profile> {
        crate::agent().host_probe().power_current(env)
    }
    fn power_set_profile(&self, env: &iyakki_shared::power::PowerEnv, p: iyakki_shared::power::Profile) -> Result<usize, String> {
        crate::agent().host_probe().power_set_profile(env, p)
    }
    fn power_fan_card(&self, env: &iyakki_shared::power::PowerEnv) -> Option<iyakki_shared::power::FanCard> {
        crate::agent().host_probe().power_fan_card(env, self.data_dir())
    }
    fn power_fan_toggle(&self, env: &iyakki_shared::power::PowerEnv) -> Result<bool, String> {
        crate::agent().host_probe().power_fan_toggle(env, self.data_dir())
    }
}

// ------------------------------------------------------------------------------------------------------------------ Computer

/// The This computer page: the per-computer profile, the firmware's startup disk and the restart-into-IYAKKI switch.
pub(crate) trait ComputerPort: PagePort {
    /// What the page shows; `None` when this install keeps no per-computer settings.
    fn computer_page(&self) -> Option<iyakki_agent_api::ComputerPage>;
    fn computer_set_sharing(&self, cat: iyakki_shared::hwprofile::Category, shared: bool) -> Result<String, String>;
    fn computer_copy_from(&self, id: &str) -> Result<String, String>;
    fn computer_reset(&self) -> Result<String, String>;
    fn computer_forget(&self, id: &str) -> Result<String, String>;
    /// "Restart through `iyakki dev reboot` starts IYAKKI once" is on.
    fn reboot_into_iyakki(&self) -> bool;
    fn set_reboot_into_iyakki(&self, on: bool) -> Result<(), String>;
    /// One line about the firmware's boot entries.
    fn startup_status_line(&self) -> String;
    /// Change the firmware's boot entries (the core asks the capability again).
    fn startup_apply(&self, a: iyakki_core_api::startupdisk::StartupAction) -> Result<String, String>;
}

impl ComputerPort for AppCx {
    fn computer_page(&self) -> Option<iyakki_agent_api::ComputerPage> {
        self.b3().computer_page()
    }
    fn computer_set_sharing(&self, cat: iyakki_shared::hwprofile::Category, shared: bool) -> Result<String, String> {
        self.b3().computer_set_sharing(cat, shared)
    }
    fn computer_copy_from(&self, id: &str) -> Result<String, String> {
        self.b3().computer_copy_from(id)
    }
    fn computer_reset(&self) -> Result<String, String> {
        self.b3().computer_reset()
    }
    fn computer_forget(&self, id: &str) -> Result<String, String> {
        self.b3().computer_forget(id)
    }
    fn reboot_into_iyakki(&self) -> bool {
        self.services.startup_reboot_into_iyakki()
    }
    fn set_reboot_into_iyakki(&self, on: bool) -> Result<(), String> {
        self.services.set_startup_reboot_into_iyakki(on)
    }
    fn startup_status_line(&self) -> String {
        self.os.startup_disk().status_line(self.services.startup_reboot_into_iyakki())
    }
    fn startup_apply(&self, a: iyakki_core_api::startupdisk::StartupAction) -> Result<String, String> {
        self.os.startup_disk().apply(a, true)
    }
}

// REAL

// -------------------------------------------------------------------------------------------------- Updates and setup wizards

/// The system updates page and the setup wizards: the store facets (update check, components, pending wizards), the update-check settings
/// (through the settings file), the person's name and policy, the compatibility lines and the wizard backend that writes to the audit log.
pub(crate) trait UpdatesPort: super::super::settings_port::SettingsDataPort {
    /// The signed-in person's name.
    fn user_name(&self) -> String;
    /// The signed-in person's policy (what the update check may do for them).
    fn policy(&self) -> iyakki_sec::policy::Policy;
    /// The store facets (the update check, the components, the wizards waiting for a person).
    fn store_api(&self) -> std::sync::Arc<dyn iyakki_agent_api::StoreFacets>;
    /// The compatibility lines the Updates page shows.
    fn compat_lines(&self) -> Vec<String>;
    /// The wizard backend of the person; its audit lines go to the audit log.
    fn wizard_backend(&self) -> std::sync::Arc<dyn iyakki_shared::wizardtools::Backend>;
}

impl UpdatesPort for AppCx {
    fn user_name(&self) -> String {
        self.session.user.clone()
    }
    fn policy(&self) -> iyakki_sec::policy::Policy {
        self.session.policy.clone()
    }
    fn store_api(&self) -> std::sync::Arc<dyn iyakki_agent_api::StoreFacets> {
        self.storeapi()
    }
    fn compat_lines(&self) -> Vec<String> {
        crate::apps::doctor_facts::compat_lines(self)
    }
    fn wizard_backend(&self) -> std::sync::Arc<dyn iyakki_shared::wizardtools::Backend> {
        let (audit, user) = (self.audit.clone(), self.session.user.clone());
        self.b3().wizard_backend(&self.session.user, std::sync::Arc::new(move |a, d| audit.record(&user, a, d)))
    }
}

// ------------------------------------------------------------------------------------------------------------------- Fake

/// The fake system port: an administrator switch, an audit list, a manual clock, and in-memory state for each page.
#[cfg(test)]
pub(crate) struct FakeSys {
    pub admin: std::sync::atomic::AtomicBool,
    pub audit_lines: std::sync::Mutex<Vec<(String, String)>>,
    pub clock: iyakki_testkit::ManualClock,
    pub dir: std::path::PathBuf,
    pub startup: std::sync::Arc<startup::Registry>,
    pub polled: std::sync::atomic::AtomicUsize,
    pub retried: std::sync::Mutex<Vec<&'static str>>,
    /// Every fake call that can be refused answers this while it is set.
    pub refuse: std::sync::Mutex<Option<String>>,
    pub cloud: std::sync::Mutex<iyakki_cloudmode::Setting>,
    pub show_ai: std::sync::atomic::AtomicBool,
    pub crash: std::sync::Arc<iyakki_core_fake::FakeCrashLog>,
    pub chat_chosen: std::sync::atomic::AtomicBool,
    pub wizards: std::sync::atomic::AtomicUsize,
    pub interrupted: std::sync::Mutex<Option<String>>,
    pub fell_back: std::sync::atomic::AtomicBool,
    pub badge: std::sync::Mutex<Option<String>>,
    pub in_memory: std::sync::atomic::AtomicBool,
    pub doctor: std::sync::atomic::AtomicBool,
    pub matrix: std::sync::Arc<FakeMatrix>,
    pub blocked_hosts: std::sync::Mutex<Vec<String>>,
    pub sources: std::sync::Mutex<Vec<(String, String)>>,
    pub levels: std::sync::Mutex<std::collections::BTreeMap<String, iyakki_shared::inbox::Level>>,
    pub camvision: std::sync::atomic::AtomicBool,
    pub disks_allowed: std::sync::atomic::AtomicBool,
    pub power_allowed: std::sync::atomic::AtomicBool,
    pub signout_close: std::sync::atomic::AtomicBool,
    pub usb: std::sync::Mutex<iyakki_core_api::device_types::storage::loss::Snapshot>,
    pub time: std::sync::Mutex<iyakki_shared::settings::TimeSettings>,
    pub face: std::sync::Mutex<(bool, bool)>,
    pub ntp: std::sync::Mutex<iyakki_shared::ntp::Status>,
    pub ntp_answer: std::sync::Mutex<String>,
    pub sync_log: std::sync::Arc<std::sync::Mutex<Vec<(Vec<String>, bool)>>>,
    pub lookups: std::sync::atomic::AtomicUsize,
    pub lookup_busy: std::sync::atomic::AtomicBool,
    pub hostname_conf: std::path::PathBuf,
    pub hostnames: std::sync::Mutex<Vec<String>>,
    pub settings: std::sync::Mutex<iyakki_shared::settings::Settings>,
    /// Holds the settings busy (every change answers `Busy`) while set.
    pub busy: std::sync::atomic::AtomicBool,
    pub caps: std::sync::Mutex<iyakki_shared::power::Caps>,
    pub profile: std::sync::Mutex<Option<iyakki_shared::power::Profile>>,
    /// The fan card: `None` when there is no fan, else whether the system runs it.
    pub fan: std::sync::Mutex<Option<bool>>,
    pub computer: std::sync::Mutex<Option<iyakki_agent_api::ComputerPage>>,
    pub comp_calls: std::sync::Mutex<Vec<String>>,
    pub reboot_into: std::sync::atomic::AtomicBool,
    pub updates_store: std::sync::Arc<iyakki_agent_api::FakeStoreFacets>,
    // FIELDS
}

#[cfg(test)]
impl FakeSys {
    pub(crate) fn new(admin: bool) -> FakeSys {
        FakeSys {
            admin: admin.into(),
            audit_lines: Default::default(),
            clock: iyakki_testkit::ManualClock::new(1_000_000),
            dir: std::path::PathBuf::from("fake-data"),
            startup: std::sync::Arc::new(startup::Registry::new(|| 0)),
            polled: 0.into(),
            retried: Default::default(),
            refuse: Default::default(),
            cloud: std::sync::Mutex::new(iyakki_cloudmode::Setting::Auto),
            show_ai: false.into(),
            crash: Default::default(),
            chat_chosen: true.into(),
            wizards: 0.into(),
            interrupted: Default::default(),
            fell_back: false.into(),
            badge: Default::default(),
            in_memory: false.into(),
            doctor: true.into(),
            matrix: Default::default(),
            blocked_hosts: Default::default(),
            sources: std::sync::Mutex::new(vec![("jobs".into(), "Finished jobs".into()), ("devices".into(), "New devices".into())]),
            levels: Default::default(),
            camvision: false.into(),
            disks_allowed: true.into(),
            power_allowed: true.into(),
            signout_close: false.into(),
            usb: Default::default(),
            time: Default::default(),
            face: Default::default(),
            ntp: Default::default(),
            ntp_answer: std::sync::Mutex::new("Set the clock.".into()),
            sync_log: Default::default(),
            lookups: 0.into(),
            lookup_busy: false.into(),
            hostname_conf: std::path::PathBuf::from("fake-data/system.conf"),
            hostnames: Default::default(),
            settings: Default::default(),
            busy: false.into(),
            caps: Default::default(),
            profile: Default::default(),
            fan: Default::default(),
            computer: Default::default(),
            comp_calls: Default::default(),
            reboot_into: false.into(),
            updates_store: Default::default(),
            // INIT
        }
    }
    pub(crate) fn audited(&self) -> Vec<(String, String)> {
        self.audit_lines.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl PagePort for FakeSys {
    fn is_admin(&self) -> bool {
        self.admin.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit_lines.lock().unwrap().push((action.to_string(), subject.to_string()));
    }
    fn data_dir(&self) -> std::path::PathBuf {
        self.dir.clone()
    }
    fn now_secs(&self) -> u64 {
        self.clock.now_secs()
    }
}

#[cfg(test)]
impl StartupPort for FakeSys {
    fn startup_rows(&self) -> Vec<Row> {
        self.startup.snapshot()
    }
    fn startup_summary(&self) -> Summary {
        self.startup.summary()
    }
    fn startup_poll(&self) {
        self.polled.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    fn startup_retry(&self, id: &'static str) {
        self.retried.lock().unwrap().push(id);
    }
}

#[cfg(test)]
impl CloudPort for FakeSys {
    fn cloud_setting(&self) -> iyakki_cloudmode::Setting {
        *self.cloud.lock().unwrap()
    }
    fn set_cloud_setting(&self, v: iyakki_cloudmode::Setting) -> Result<(), String> {
        if let Some(e) = self.refuse.lock().unwrap().clone() {
            return Err(e);
        }
        *self.cloud.lock().unwrap() = v;
        Ok(())
    }
    fn show_all_ai(&self) -> bool {
        self.show_ai.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn set_show_all_ai(&self, v: bool) -> Result<(), String> {
        if let Some(e) = self.refuse.lock().unwrap().clone() {
            return Err(e);
        }
        self.show_ai.store(v, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn ai_machine_summary(&self) -> String {
        "fake machine".into()
    }
}

#[cfg(test)]
impl CrashPort for FakeSys {
    fn crashlog(&self) -> std::sync::Arc<dyn iyakki_core_api::crashlog::CrashLog> {
        self.crash.clone()
    }
    fn home_for_writes(&self) -> std::path::PathBuf {
        std::path::PathBuf::from("fake-home")
    }
}

#[cfg(test)]
impl OverviewPort for FakeSys {
    fn chat_chosen(&self) -> bool {
        self.chat_chosen.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn pending_wizard_count(&self) -> usize {
        self.wizards.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn update_interrupted_text(&self, _: &std::path::Path) -> Option<String> {
        self.interrupted.lock().unwrap().clone()
    }
    fn update_fell_back(&self, _: &std::path::Path) -> bool {
        self.fell_back.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn update_badge(&self) -> Option<String> {
        self.badge.lock().unwrap().clone()
    }
    fn data_in_memory(&self, _: &std::path::Path) -> bool {
        self.in_memory.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn allows_doctor(&self) -> bool {
        self.doctor.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// A Matrix connector in memory: one account at most, its rooms, the copy switch, and the calls it was given.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct FakeMatrix {
    pub accounts: std::sync::Mutex<Vec<iyakki_agent_api::MatrixAccount>>,
    pub rooms: std::sync::Mutex<Vec<iyakki_agent_api::MatrixRoom>>,
    pub mirror: std::sync::atomic::AtomicBool,
    pub unread: std::sync::Mutex<Vec<(String, usize)>>,
    /// Every call that can fail answers this while it is set.
    pub fail: std::sync::Mutex<Option<String>>,
    pub calls: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl FakeMatrix {
    pub(crate) fn with_account() -> std::sync::Arc<FakeMatrix> {
        let m = FakeMatrix::default();
        m.accounts.lock().unwrap().push(iyakki_agent_api::MatrixAccount { id: "m1".into(), label: "Home".into(), user_id: "@alice:fake.test".into(), homeserver: "https://fake.test".into() });
        m.rooms.lock().unwrap().push(iyakki_agent_api::MatrixRoom { name: "main".into(), mirror: true });
        std::sync::Arc::new(m)
    }
    fn err(&self) -> Result<(), String> {
        self.fail.lock().unwrap().clone().map_or(Ok(()), Err)
    }
    pub(crate) fn called(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl iyakki_agent_api::MatrixService for FakeMatrix {
    fn accounts(&self) -> Vec<iyakki_agent_api::MatrixAccount> {
        self.accounts.lock().unwrap().clone()
    }
    fn rooms(&self, _account: &str) -> Vec<iyakki_agent_api::MatrixRoom> {
        self.rooms.lock().unwrap().clone()
    }
    fn mirror_on(&self, _account: &str) -> bool {
        self.mirror.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn set_mirror(&self, _account: &str, on: bool) -> Result<(), String> {
        self.err()?;
        self.mirror.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn set_room_mirror(&self, _account: &str, room: &str, on: bool) -> Result<iyakki_agent_api::MatrixRoom, String> {
        self.err()?;
        let mut rooms = self.rooms.lock().unwrap();
        let r = rooms.iter_mut().find(|r| r.name == room).ok_or("no such room")?;
        r.mirror = on;
        Ok(r.clone())
    }
    fn unread_by_room(&self, _account: &str) -> Vec<(String, usize)> {
        self.unread.lock().unwrap().clone()
    }
    fn add(&self, label: &str, homeserver: &str, auth: iyakki_agent_api::MatrixAuth, room_id: &str) -> Result<iyakki_agent_api::MatrixAccount, String> {
        self.err()?;
        let who = match auth {
            iyakki_agent_api::MatrixAuth::Password { user, .. } => user,
            iyakki_agent_api::MatrixAuth::Token(_) => "token".to_string(),
        };
        self.calls.lock().unwrap().push(format!("add {label} {homeserver} {who} {room_id}"));
        let a = iyakki_agent_api::MatrixAccount { id: "m1".into(), label: label.into(), user_id: format!("@{who}:fake.test"), homeserver: homeserver.into() };
        self.accounts.lock().unwrap().push(a.clone());
        if !room_id.is_empty() {
            self.rooms.lock().unwrap().push(iyakki_agent_api::MatrixRoom { name: room_id.into(), mirror: true });
        }
        Ok(a)
    }
    fn test_send(&self, account: &str) -> Result<String, String> {
        self.err()?;
        self.calls.lock().unwrap().push(format!("test_send {account}"));
        Ok("sent".into())
    }
    fn remove(&self, account: &str) -> Result<(), String> {
        self.err()?;
        self.calls.lock().unwrap().push(format!("remove {account}"));
        self.accounts.lock().unwrap().retain(|a| a.id != account);
        Ok(())
    }
}

#[cfg(test)]
impl MatrixPort for FakeSys {
    fn matrix(&self) -> std::sync::Arc<dyn iyakki_agent_api::MatrixService> {
        self.matrix.clone()
    }
    fn matrix_check_homeserver(&self, addr: &str) -> Result<String, String> {
        if addr.trim().is_empty() {
            return Err("Enter the address of the homeserver.".into());
        }
        Ok(addr.trim().to_string())
    }
    fn host_of(&self, url: &str) -> String {
        url.trim_start_matches("https://").trim_start_matches("http://").split('/').next().unwrap_or("").to_string()
    }
    fn allows_host(&self, host: &str) -> bool {
        !self.blocked_hosts.lock().unwrap().iter().any(|h| h == host)
    }
}

#[cfg(test)]
impl NotifsPort for FakeSys {
    fn notif_sources(&self) -> Vec<(String, String)> {
        self.sources.lock().unwrap().clone()
    }
    fn notif_levels(&self) -> std::collections::BTreeMap<String, iyakki_shared::inbox::Level> {
        self.levels.lock().unwrap().clone()
    }
    fn notif_level(&self, source: &str) -> iyakki_shared::inbox::Level {
        self.levels.lock().unwrap().get(source).copied().unwrap_or(iyakki_shared::inbox::Level::Show)
    }
    fn notif_set_level(&self, source: &str, level: iyakki_shared::inbox::Level) -> Result<(), String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        self.levels.lock().unwrap().insert(source.to_string(), level);
        Ok(())
    }
    fn camvision_on(&self) -> bool {
        self.camvision.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn camvision_set(&self, on: bool) -> Result<(), String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        self.camvision.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
impl RestartPort for FakeSys {
    fn allows_power(&self) -> bool {
        self.power_allowed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
impl StoragePort for FakeSys {
    fn allows_disks(&self) -> bool {
        self.disks_allowed.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn home_root(&self, _: &std::path::Path) -> String {
        "/home/fake".into()
    }
    fn disks_media_dir(&self, home: &str) -> String {
        format!("{home}/media")
    }
    fn disks_signout_pref(&self, _: &str) -> bool {
        self.signout_close.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn disks_set_signout_pref(&self, _: &str, on: bool) -> Result<(), String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        self.signout_close.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn usb_loss_snapshot(&self) -> iyakki_core_api::device_types::storage::loss::Snapshot {
        self.usb.lock().unwrap().clone()
    }
    fn set_usb_compat(&self, on: bool, usb_id: &str) -> Result<String, String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        self.usb.lock().unwrap().compat = on;
        Ok(format!("compat {on} for {usb_id}"))
    }
}

#[cfg(test)]
impl TimePort for FakeSys {
    fn time_settings(&self) -> iyakki_shared::settings::TimeSettings {
        self.time.lock().unwrap().clone()
    }
    fn time_update(&self, f: &mut dyn FnMut(&mut iyakki_shared::settings::TimeSettings)) -> Result<(), String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        f(&mut self.time.lock().unwrap());
        Ok(())
    }
    fn contract_setting(&self, _: &str) -> Option<iyakki_core_api::contract::SettingValue> {
        None
    }
    fn clock_face(&self) -> (bool, bool) {
        *self.face.lock().unwrap()
    }
    fn set_clock_face(&self, hour12: bool, seconds: bool) {
        *self.face.lock().unwrap() = (hour12, seconds);
    }
    fn ntp_servers_for(&self, custom: &str) -> Vec<String> {
        if custom.is_empty() { vec!["time.fake.test".into()] } else { vec![custom.into()] }
    }
    fn ntp_valid_server(&self, s: &str) -> bool {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    }
    fn ntp_status(&self) -> iyakki_shared::ntp::Status {
        self.ntp.lock().unwrap().clone()
    }
    fn ntp_sync_job(&self, servers: Vec<String>, allow_back: bool, setter: super::time::ClockSetter) -> Box<dyn FnOnce() -> String + Send> {
        let answer = self.ntp_answer.lock().unwrap().clone();
        let log = self.sync_log.clone();
        Box::new(move || {
            log.lock().unwrap().push((servers, allow_back));
            let _ = setter(1_800_000_000.0);
            answer
        })
    }
    fn zone_lookup_in_background(&self, on_change: std::sync::Arc<dyn Fn(&str) + Send + Sync>) -> bool {
        self.lookups.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.lookup_busy.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        on_change("Europe/Fake");
        true
    }
}

#[cfg(test)]
impl HostNamePort for FakeSys {
    fn hostname_conf_path(&self) -> std::path::PathBuf {
        self.hostname_conf.clone()
    }
    fn set_hostname(&self, name: &str, _: &std::path::Path) -> Result<(), String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        self.hostnames.lock().unwrap().push(name.to_string());
        Ok(())
    }
}

#[cfg(test)]
impl super::super::settings_port::SettingsDataPort for FakeSys {
    fn settings_get(&self) -> iyakki_shared::settings::Settings {
        self.settings.lock().unwrap().clone()
    }
    fn settings_update(&self, f: &mut dyn FnMut(&mut iyakki_shared::settings::Settings)) -> Result<(), super::super::settings_port::SettingsWriteError> {
        use super::super::settings_port::SettingsWriteError;
        if self.busy.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SettingsWriteError::Busy);
        }
        let mut cur = self.settings.lock().unwrap();
        let mut next = cur.clone();
        f(&mut next);
        next.validate().map_err(|e| SettingsWriteError::Refused(e.to_string()))?;
        *cur = next;
        Ok(())
    }
}

#[cfg(test)]
impl PowerPort for FakeSys {
    fn power_caps(&self, _: &iyakki_shared::power::PowerEnv) -> iyakki_shared::power::Caps {
        self.caps.lock().unwrap().clone()
    }
    fn power_current(&self, _: &iyakki_shared::power::PowerEnv) -> Option<iyakki_shared::power::Profile> {
        *self.profile.lock().unwrap()
    }
    fn power_set_profile(&self, _: &iyakki_shared::power::PowerEnv, p: iyakki_shared::power::Profile) -> Result<usize, String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        *self.profile.lock().unwrap() = Some(p);
        Ok(1)
    }
    fn power_fan_card(&self, _: &iyakki_shared::power::PowerEnv) -> Option<iyakki_shared::power::FanCard> {
        self.fan.lock().unwrap().map(|enabled| iyakki_shared::power::FanCard { enabled, line: "fake fans".into() })
    }
    fn power_fan_toggle(&self, _: &iyakki_shared::power::PowerEnv) -> Result<bool, String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        let mut f = self.fan.lock().unwrap();
        let on = !f.unwrap_or(false);
        *f = Some(on);
        Ok(on)
    }
}

#[cfg(test)]
impl ComputerPort for FakeSys {
    fn computer_page(&self) -> Option<iyakki_agent_api::ComputerPage> {
        self.computer.lock().unwrap().clone()
    }
    fn computer_set_sharing(&self, cat: iyakki_shared::hwprofile::Category, shared: bool) -> Result<String, String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        self.comp_calls.lock().unwrap().push(format!("sharing {}={shared}", cat.key()));
        Ok("Changed.".into())
    }
    fn computer_copy_from(&self, id: &str) -> Result<String, String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        self.comp_calls.lock().unwrap().push(format!("copy {id}"));
        Ok(format!("Copied from {id}."))
    }
    fn computer_reset(&self) -> Result<String, String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        self.comp_calls.lock().unwrap().push("reset".into());
        Ok("Reset.".into())
    }
    fn computer_forget(&self, id: &str) -> Result<String, String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        self.comp_calls.lock().unwrap().push(format!("forget {id}"));
        Ok(format!("Forgot {id}."))
    }
    fn reboot_into_iyakki(&self) -> bool {
        self.reboot_into.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn set_reboot_into_iyakki(&self, on: bool) -> Result<(), String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        self.reboot_into.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn startup_status_line(&self) -> String {
        "fake firmware order".into()
    }
    fn startup_apply(&self, a: iyakki_core_api::startupdisk::StartupAction) -> Result<String, String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)?;
        self.comp_calls.lock().unwrap().push(format!("startup {a:?}"));
        Ok("Done.".into())
    }
}

// FAKEIMPL

#[cfg(test)]
impl UpdatesPort for FakeSys {
    fn user_name(&self) -> String {
        "owner".into()
    }
    fn policy(&self) -> iyakki_sec::policy::Policy {
        if self.admin.load(std::sync::atomic::Ordering::SeqCst) {
            iyakki_sec::policy::Role::Admin.template()
        } else {
            iyakki_sec::policy::Role::User.template()
        }
    }
    fn store_api(&self) -> std::sync::Arc<dyn iyakki_agent_api::StoreFacets> {
        self.updates_store.clone()
    }
    fn compat_lines(&self) -> Vec<String> {
        vec!["This computer runs the current system.".into()]
    }
    fn wizard_backend(&self) -> std::sync::Arc<dyn iyakki_shared::wizardtools::Backend> {
        std::sync::Arc::new(NoWizards)
    }
}

/// A wizard backend with no wizards installed and nothing pending.
#[cfg(test)]
pub(crate) struct NoWizards;

#[cfg(test)]
impl iyakki_shared::wizardtools::Backend for NoWizards {
    fn installed(&self) -> Vec<(String, String, iyakki_core_api::wizard::WizardSpec)> {
        Vec::new()
    }
    fn pending(&self) -> Vec<(String, String, String, String)> {
        Vec::new()
    }
    fn clear_pending(&self, _app: &str, _wizard: &str) {}
    fn env(&self) -> std::sync::Arc<dyn iyakki_shared::wizardrun::Env> {
        std::sync::Arc::new(NoEnv)
    }
    fn run(&self) -> std::sync::Arc<std::sync::Mutex<Option<(String, iyakki_shared::wizardrun::Run)>>> {
        std::sync::Arc::new(std::sync::Mutex::new(None))
    }
    fn audit(&self, _what: &str, _detail: &str) {}
}

#[cfg(test)]
struct NoEnv;

#[cfg(test)]
impl iyakki_shared::wizardrun::Env for NoEnv {
    fn check(&self, _check: &str, _param: &str) -> Result<(), String> {
        Ok(())
    }
    fn migrate(&self, _reference: &str, _from: Option<&str>, _to: Option<&str>, _answers: &std::collections::BTreeMap<String, serde_json::Value>) -> Result<String, String> {
        Ok(String::new())
    }
}
