//! What the Settings pages of lane ST-D (display, sound, devices, appearance, accessibility) ask of the rest of the system
//! (docs/development/SETTINGS-EXTRACTION-PLAN.md, section 3; the app standard is APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! `settings_port.rs` is frozen and holds the two shared traits (`PagePort`, `SettingsDataPort`). This file adds, for this lane only:
//!
//! - [`UserPort`]: who is signed in (the name that names the person's own files), on top of `PagePort`;
//! - one small trait per page that needs more (`AccessPort`, ...), each naming exactly the calls its page makes.
//!
//! The real implementation of every trait is `impl ... for AppCx` here, the one place of this lane that reads the session, the services,
//! the core and the agent. The fake is [`FakeDevicePort`] under `#[cfg(test)]`: in-memory answers and a record of what was asked, a private
//! data folder per test, a manual clock; nothing sleeps and nothing reaches the hardware.

#[cfg(test)]
use std::path::PathBuf;

use super::settings_port::{PagePort, SettingsDataPort};
#[cfg(test)]
use super::settings_port::SettingsWriteError;
use super::AppCx;

/// Who is looking: the name of the signed-in person (it names their own files in the data folder).
pub(crate) trait UserPort: PagePort {
    fn user(&self) -> String;
}

impl UserPort for AppCx {
    fn user(&self) -> String {
        self.session.user.clone()
    }
}

// ---- Accessibility -------------------------------------------------------------------------------------------------------------------

/// What the Accessibility page asks besides the person's own files: is a reading voice installed, say a test sentence, what the braille
/// display answers.
pub(crate) trait AccessPort: UserPort {
    /// A reading voice is installed (the screen reader can speak).
    fn voice_ready(&self) -> bool;
    /// Say `text` with the system voice.
    fn speak_test(&self, text: &str) -> Result<(), String>;
    /// What the braille link says about the connected display.
    fn braille_status(&self) -> String;
}

impl AccessPort for AppCx {
    fn voice_ready(&self) -> bool {
        self.b4().voice_ready()
    }
    fn speak_test(&self, text: &str) -> Result<(), String> {
        self.services.system_speaker().say(text).map_err(|e| e.to_string())
    }
    fn braille_status(&self) -> String {
        self.core.braille().status()
    }
}

// ---- Displays ------------------------------------------------------------------------------------------------------------------------

/// What the Displays page asks of the core: the monitors that answer over DDC/CI, and the machine's memory (the Screen features gate).
pub(crate) trait DisplaysPort: UserPort {
    /// Every screen with a DDC channel (a slow read: only when the person asks).
    fn monitors_list(&self) -> Vec<iyakki_core_api::MonitorInfo>;
    /// Choose the one screen that may be written to (`None`: none).
    fn monitor_choose(&self, id: Option<&str>) -> Result<(), String>;
    /// Set the brightness (1 to 100) of the chosen screen; the value read back.
    fn monitor_brightness(&self, percent: u8) -> Result<u8, String>;
    /// The machine's memory now.
    fn mem_facts(&self) -> crate::memgate::MemFacts;
}

impl DisplaysPort for AppCx {
    fn monitors_list(&self) -> Vec<iyakki_core_api::MonitorInfo> {
        self.core.monitors().list()
    }
    fn monitor_choose(&self, id: Option<&str>) -> Result<(), String> {
        self.core.monitors().choose(id)
    }
    fn monitor_brightness(&self, percent: u8) -> Result<u8, String> {
        self.core.monitors().set_brightness(percent)
    }
    fn mem_facts(&self) -> crate::memgate::MemFacts {
        crate::memgate::read_mem_os(&*self.os)
    }
}

// ---- System sounds -------------------------------------------------------------------------------------------------------------------

/// What the System sounds page asks: the cue settings (read and change), whether IYAKKI is muted, and a preview that runs off the page's
/// thread.
pub(crate) trait SystemSoundsPort: UserPort {
    fn system_sounds(&self) -> iyakki_shared::syssounds::SystemSounds;
    /// Change the cue settings and save them (the closure runs once); the new settings, or why not.
    fn update_system_sounds(&self, f: &mut dyn FnMut(&mut iyakki_shared::syssounds::SystemSounds)) -> Result<iyakki_shared::syssounds::SystemSounds, String>;
    /// IYAKKI's sound is muted (the cues are silent then).
    fn sound_muted(&self) -> bool;
    /// The work of a preview of `cue`, to run on a job thread (it plays the cue and answers with the sentence to show).
    fn preview_job(&self, cue: iyakki_core_api::soundtypes::Cue) -> Box<dyn FnOnce() -> Result<String, String> + Send>;
}

impl SystemSoundsPort for AppCx {
    fn system_sounds(&self) -> iyakki_shared::syssounds::SystemSounds {
        self.services.system_sounds()
    }
    fn update_system_sounds(&self, f: &mut dyn FnMut(&mut iyakki_shared::syssounds::SystemSounds)) -> Result<iyakki_shared::syssounds::SystemSounds, String> {
        self.services.update_system_sounds_with(f)
    }
    fn sound_muted(&self) -> bool {
        self.sound_settings().muted
    }
    fn preview_job(&self, cue: iyakki_core_api::soundtypes::Cue) -> Box<dyn FnOnce() -> Result<String, String> + Send> {
        let svc = self.services.clone();
        let out = svc.cue_out();
        Box::new(move || svc.preview_cue(cue, out))
    }
}

// ---- Home layout ---------------------------------------------------------------------------------------------------------------------

/// What the Home layout page asks besides the person's own layout file: may this person manage users (the tiles listed depend on it), and
/// the tile apps of the store this person sees.
pub(crate) trait HomeLayoutPort: UserPort {
    fn can_manage_users(&self) -> bool;
    fn store_tiles(&self) -> Vec<iyakki_shared::store_tilehub::TileEntry>;
}

impl HomeLayoutPort for AppCx {
    fn can_manage_users(&self) -> bool {
        self.session.policy.manage_users
    }
    fn store_tiles(&self) -> Vec<iyakki_shared::store_tilehub::TileEntry> {
        self.storeapi().tiles(&self.session.policy, &self.session.user)
    }
}

// ---- Screen saver --------------------------------------------------------------------------------------------------------------------

/// What the Screen saver page asks besides the person's own files and the machine's settings: is this a guest (no password to lock with).
pub(crate) trait ScreenSaverPort: UserPort + SettingsDataPort {
    fn is_guest(&self) -> bool;
}

impl ScreenSaverPort for AppCx {
    fn is_guest(&self) -> bool {
        self.session.role == iyakki_sec::policy::Role::Guest
    }
}

// ---- the machine's settings file on the real context ----------------------------------------------------------------------------------
// (`SettingsDataPort for AppCx` is in settings_port.rs; the fake below answers it from memory.)

// ---- Appearance ----------------------------------------------------------------------------------------------------------------------

/// What the Appearance page asks besides the person's own look file: the apps this person may pin, and which apps they chose the SDK
/// version of (kept in the core's app data).
pub(crate) trait AppearancePort: UserPort {
    /// The apps this person may pin on the home screen, in tile order: (id, label).
    fn pinnable(&self) -> Vec<(String, String)>;
    /// The person chose the SDK version of this app.
    fn sdk_app_chosen(&self, app: &str) -> bool;
    /// Choose or leave the SDK version of an app; the sentence to show, or why not.
    fn sdk_app_set(&self, app: &str, on: bool) -> Result<String, String>;
}

impl AppearancePort for AppCx {
    fn pinnable(&self) -> Vec<(String, String)> {
        super::pinnable(&self.session.policy).into_iter().map(|(a, l)| (a.to_string(), l.to_string())).collect()
    }
    fn sdk_app_chosen(&self, app: &str) -> bool {
        super::sdk_apps::chosen(self, app)
    }
    fn sdk_app_set(&self, app: &str, on: bool) -> Result<String, String> {
        super::sdk_apps::set_chosen(&*self.core, &self.session.user, app, on)
    }
}

// ---- Sound ---------------------------------------------------------------------------------------------------------------------------

/// A piece of work for a job thread: it answers with the sentence to show, or why not.
pub(crate) type SoundJob = Box<dyn FnOnce() -> Result<String, String> + Send>;

/// What the Sound page asks: the sound settings (read and change), the voice settings (the microphone choice), the sound server, the
/// plays that take time (test sound, patterns, the hardware volume check, the microphone test), the Bluetooth route and the T2 microphone.
pub(crate) trait SoundPort: UserPort {
    fn sound_settings(&self) -> iyakki_shared::audioout::SoundSettings;
    /// The sound card's line while it is still starting (`None` when it is ready, or there is no watcher).
    fn refresh_starting(&self) -> Option<String>;
    /// Change the sound settings and save them (the closure runs once); the new settings, or why not.
    fn update_sound(&self, f: &mut dyn FnMut(&mut iyakki_shared::audioout::SoundSettings)) -> Result<iyakki_shared::audioout::SoundSettings, String>;
    fn voice_settings(&self) -> iyakki_shared::voice::VoiceSettings;
    /// Choose the microphone (empty: automatic).
    fn set_voice_mic(&self, id: &str) -> Result<(), String>;
    /// A sound server runs (it chooses the output itself in automatic mode).
    fn sound_server_running(&self) -> bool;
    /// The work of a test chime on `device` (`None`: the default device).
    fn test_sound_job(&self, device: Option<String>) -> SoundJob;
    /// The work of the stereo test or the channel walk.
    fn test_pattern_job(&self, walk: bool) -> SoundJob;
    /// The work of the two tones that tell whether a card's hardware volume works.
    fn hw_volume_check_job(&self, card: u32, device: u32, control: String) -> SoundJob;
    /// The Bluetooth sound route: sending, and the paired headphones and speakers.
    fn bluetooth_sound_view(&self) -> iyakki_agent_api::BtSoundView;
    fn bluetooth_sound_start(&self, address: &str) -> Result<String, String>;
    fn bluetooth_sound_stop(&self) -> Result<String, String>;
    /// The T2 sound chip's built-in microphone: absent, hidden, allowed.
    fn t2_mic(&self) -> iyakki_shared::voicedesk::T2Mic;
    fn set_t2_capture(&self, on: bool) -> Result<(), String>;
    /// The sound inputs the machine has.
    fn list_mics(&self) -> Vec<iyakki_shared::voicedesk::Mic>;
    /// The record command aimed at `mic`.
    fn record_cmd_for(&self, base: &str, mic: Option<&iyakki_shared::voicedesk::Mic>) -> String;
    /// The work of listening for three seconds with `cmd` and saying how loud it was (`label` names the input).
    fn mic_test_job(&self, label: String, cmd: String) -> SoundJob;
}

impl SoundPort for AppCx {
    fn sound_settings(&self) -> iyakki_shared::audioout::SoundSettings {
        AppCx::sound_settings(self)
    }
    fn refresh_starting(&self) -> Option<String> {
        self.core.sound().refresh_starting()
    }
    fn update_sound(&self, f: &mut dyn FnMut(&mut iyakki_shared::audioout::SoundSettings)) -> Result<iyakki_shared::audioout::SoundSettings, String> {
        self.services.update_sound_with(f)
    }
    fn voice_settings(&self) -> iyakki_shared::voice::VoiceSettings {
        AppCx::voice_settings(self)
    }
    fn set_voice_mic(&self, id: &str) -> Result<(), String> {
        self.services.set_voice_mic(id)
    }
    fn sound_server_running(&self) -> bool {
        self.core.sound().server_running()
    }
    fn test_sound_job(&self, device: Option<String>) -> SoundJob {
        let svc = self.services.clone();
        Box::new(move || svc.test_sound(device.as_deref()))
    }
    fn test_pattern_job(&self, walk: bool) -> SoundJob {
        let svc = self.services.clone();
        Box::new(move || svc.test_pattern(walk))
    }
    fn hw_volume_check_job(&self, card: u32, device: u32, control: String) -> SoundJob {
        let svc = self.services.clone();
        Box::new(move || svc.hw_volume_check(card, device, &control))
    }
    fn bluetooth_sound_view(&self) -> iyakki_agent_api::BtSoundView {
        crate::agent().sound_probe().bluetooth_sound_view()
    }
    fn bluetooth_sound_start(&self, address: &str) -> Result<String, String> {
        crate::agent().sound_probe().bluetooth_sound_start(AppCx::data_dir(self), address)
    }
    fn bluetooth_sound_stop(&self) -> Result<String, String> {
        crate::agent().sound_probe().bluetooth_sound_stop(AppCx::data_dir(self))
    }
    fn t2_mic(&self) -> iyakki_shared::voicedesk::T2Mic {
        self.agent.sound_probe().t2_mic(&self.agent.voicedesk_proc_root())
    }
    fn set_t2_capture(&self, on: bool) -> Result<(), String> {
        self.agent.voicedesk_set_t2_capture(&self.agent.voicedesk_proc_root(), on)
    }
    fn list_mics(&self) -> Vec<iyakki_shared::voicedesk::Mic> {
        crate::agent().voicedesk_list_mics(&crate::agent().voicedesk_proc_root())
    }
    fn record_cmd_for(&self, base: &str, mic: Option<&iyakki_shared::voicedesk::Mic>) -> String {
        self.agent.voicedesk_record_cmd_for(base, mic)
    }
    fn mic_test_job(&self, label: String, cmd: String) -> SoundJob {
        Box::new(move || {
            let rec = crate::recorders().start(&cmd, iyakki_shared::voicedesk::MAX_RECORD_SECS)?;
            std::thread::sleep(std::time::Duration::from_millis(3000));
            let pcm = rec.stop()?;
            let pk = iyakki_shared::voice::peak(&pcm);
            if pk < 150 {
                Err(format!("{label} opened but it is silent (peak {pk}). Check that it is not muted or too quiet."))
            } else {
                Ok(format!("{label} works: peak level {pk} of 32767."))
            }
        })
    }
}

// ---- Keyboard ------------------------------------------------------------------------------------------------------------------------

/// What the Keyboard page asks besides the machine's settings file and the sound settings: the Apple keyboard driver's state, the touch
/// screen probes and resets, and whether a sound output exists.
pub(crate) trait KeyboardPort: SettingsDataPort + SoundPort {
    fn apple_state(&self, sys: &std::path::Path) -> iyakki_shared::keyboard::AppleState;
    fn touch_panel_names(&self, sys: &std::path::Path) -> Vec<String>;
    fn touch_recover_status(&self) -> String;
    /// The next step of the touch-screen probe ladder.
    fn touch_next_level(&self) -> u8;
    /// The work of probing the touch screen at `level` (the words for the person).
    fn touch_probe_job(&self, sys: std::path::PathBuf, level: u8) -> Box<dyn FnOnce() -> String + Send>;
    /// The work of resetting the touch screen now.
    fn touch_reset_job(&self) -> Box<dyn FnOnce() -> String + Send>;
    /// The machine has a sound output.
    fn sound_output_present(&self) -> bool;
}

impl KeyboardPort for AppCx {
    fn apple_state(&self, sys: &std::path::Path) -> iyakki_shared::keyboard::AppleState {
        self.agent.keyboard_apple().apple_state(sys)
    }
    fn touch_panel_names(&self, sys: &std::path::Path) -> Vec<String> {
        self.agent.helpers_b4().touch_panel_names(sys)
    }
    fn touch_recover_status(&self) -> String {
        self.agent.touchrecover_status()
    }
    fn touch_next_level(&self) -> u8 {
        self.agent.touchprobe_next_level()
    }
    fn touch_probe_job(&self, sys: std::path::PathBuf, level: u8) -> Box<dyn FnOnce() -> String + Send> {
        let agent = self.agent.clone();
        Box::new(move || agent.helpers_b4().touch_probe(&sys, level))
    }
    fn touch_reset_job(&self) -> Box<dyn FnOnce() -> String + Send> {
        let up = self.agent.doctor_uptime_secs();
        let agent = self.agent.clone();
        Box::new(move || agent.helpers_b4().touch_reset_now(up))
    }
    fn sound_output_present(&self) -> bool {
        crate::fnkeys::sound_present(&*self.core)
    }
}

// ---- USB-C direct link ---------------------------------------------------------------------------------------------------------------

/// What the USB-C page asks: the machine's view of the cable (the `Env` of the direct-link engine; `None` when this core has none).
pub(crate) trait UsbcPort: UserPort {
    fn direct_link_env(&self) -> Option<std::sync::Arc<dyn iyakki_shared::directlink::Env>>;
}

impl UsbcPort for AppCx {
    fn direct_link_env(&self) -> Option<std::sync::Arc<dyn iyakki_shared::directlink::Env>> {
        let home = AppCx::data_dir(self).join("devmode").join("client").join(&self.session.user);
        crate::agent().direct_link_b8().real_env(AppCx::data_dir(self), &home)
    }
}

// ---- Developer mode ------------------------------------------------------------------------------------------------------------------

/// What the Developer page asks: the person's policy (every developer-mode call names it), the developer-mode service, the WASM tile
/// switch, the UI socket and resume switches of the core, and the machine's addresses. The WASM switch is kept in the settings file.
pub(crate) trait DeveloperPort: UsbcPort + SettingsDataPort {
    fn policy(&self) -> iyakki_sec::policy::Policy;
    fn devmode(&self) -> std::sync::Arc<dyn iyakki_agent_api::DevModeApi>;
    fn wasm_tiles_enabled(&self) -> bool;
    fn ui_socket_enabled(&self) -> bool;
    fn set_ui_socket(&self, on: bool) -> Result<String, String>;
    fn resume_switch_on(&self) -> bool;
    fn set_resume_switch(&self, on: bool) -> Result<(), String>;
    /// (interface, address) of the machine.
    fn ipv4_addresses(&self) -> Vec<(String, String)>;
}

impl DeveloperPort for AppCx {
    fn policy(&self) -> iyakki_sec::policy::Policy {
        self.session.policy.clone()
    }
    fn devmode(&self) -> std::sync::Arc<dyn iyakki_agent_api::DevModeApi> {
        crate::agent().host_probe().devmode(AppCx::data_dir(self))
    }
    fn wasm_tiles_enabled(&self) -> bool {
        self.storeapi().wasm_tiles_enabled()
    }
    fn ui_socket_enabled(&self) -> bool {
        self.core.ui_socket().enabled()
    }
    fn set_ui_socket(&self, on: bool) -> Result<String, String> {
        self.core.ui_socket().set_enabled(on)
    }
    fn resume_switch_on(&self) -> bool {
        self.core.session_resume().switch_on()
    }
    fn set_resume_switch(&self, on: bool) -> Result<(), String> {
        self.core.session_resume().set_switch(on)
    }
    fn ipv4_addresses(&self) -> Vec<(String, String)> {
        self.os.net().ipv4_addresses().unwrap_or_default()
    }
}

// ---- Dev cluster ---------------------------------------------------------------------------------------------------------------------

/// What the Dev cluster page asks: the managed k3s cluster, the vault, the download streamer, the person's role and read policy, whether a
/// program is on the path, the audit sink for the supervisor, and the two panels that are built from the real services (Platform
/// services and Clusters: the fake has none, and the page then says they are not available).
pub(crate) trait DevClusterPort: UserPort {
    fn k3sdev(&self) -> std::sync::Arc<dyn iyakki_agent_api::DevCluster>;
    fn vault(&self) -> std::sync::Arc<dyn iyakki_agent_api::VaultRead>;
    fn streamer(&self) -> Option<std::sync::Arc<dyn iyakki_proxy::Streamer>>;
    fn role(&self) -> iyakki_sec::policy::Role;
    /// The person's account may read this path.
    fn allows_read(&self, path: &str) -> bool;
    /// A program is found on the path (`podman`, `docker`).
    fn on_path(&self, program: &str) -> bool;
    fn b5(&self) -> std::sync::Arc<dyn iyakki_agent_api::ServicesFacetB5>;
    /// Where the supervisor writes its audit lines: (user, action, detail).
    fn audit_sink(&self) -> std::sync::Arc<dyn Fn(&str, &str, &str) + Send + Sync>;
    /// The platform services of this person, run through `run` against `env`, and the handles its panel needs.
    fn platform(&self, run: std::sync::Arc<dyn iyakki_shared::wsexec::Runner>, env: iyakki_shared::devcluster::Env) -> Option<(std::sync::Arc<dyn iyakki_agent_api::PlatformApi>, iyakki_agent_api::PlatformHandles)>;
    /// The registry of local and remote clusters.
    fn fleet(&self) -> Option<std::sync::Arc<dyn iyakki_agent_api::FleetApi>>;
}

impl DevClusterPort for AppCx {
    fn k3sdev(&self) -> std::sync::Arc<dyn iyakki_agent_api::DevCluster> {
        AppCx::k3sdev(self)
    }
    fn vault(&self) -> std::sync::Arc<dyn iyakki_agent_api::VaultRead> {
        AppCx::vault(self)
    }
    fn streamer(&self) -> Option<std::sync::Arc<dyn iyakki_proxy::Streamer>> {
        AppCx::streamer(self)
    }
    fn role(&self) -> iyakki_sec::policy::Role {
        self.session.role
    }
    fn allows_read(&self, path: &str) -> bool {
        self.session.policy.allows_read(&self.session.user, path)
    }
    fn on_path(&self, program: &str) -> bool {
        self.agent.wsexec_on_path(program)
    }
    fn b5(&self) -> std::sync::Arc<dyn iyakki_agent_api::ServicesFacetB5> {
        AppCx::b5(self)
    }
    fn audit_sink(&self) -> std::sync::Arc<dyn Fn(&str, &str, &str) + Send + Sync> {
        let audit = self.audit.clone();
        std::sync::Arc::new(move |u, a, d| audit.record(u, a, d))
    }
    fn platform(&self, run: std::sync::Arc<dyn iyakki_shared::wsexec::Runner>, env: iyakki_shared::devcluster::Env) -> Option<(std::sync::Arc<dyn iyakki_agent_api::PlatformApi>, iyakki_agent_api::PlatformHandles)> {
        Some((crate::apps::platform_services::platform_for(self, run, env), self.platform_handles()))
    }
    fn fleet(&self) -> Option<std::sync::Arc<dyn iyakki_agent_api::FleetApi>> {
        Some(self.services.fleet() as std::sync::Arc<dyn iyakki_agent_api::FleetApi>)
    }
}

// ---- Git and SSH keys ----------------------------------------------------------------------------------------------------------------

/// What the Git page asks: the person's keys and connections (`GitKeys` on their vault), what their account may read and reach, the
/// environment of the connection wizard, and the Gitea quick install. It also opens the Platform services page (a dev-cluster call).
pub(crate) trait GitPort: DevClusterPort {
    /// This person's key store, connections and known hosts.
    fn git_keys(&self) -> iyakki_shared::gitkeys::GitKeys;
    /// The person's account may read this file.
    fn may_read(&self, path: &str) -> bool;
    /// The person's account may reach this host (the network policy).
    fn may_reach(&self, host: &str) -> bool;
    /// What the connection wizard works with (`http` is a test's own answer for the host's API).
    fn wizard_env(&self, keys: std::sync::Arc<iyakki_shared::gitkeys::GitKeys>, http: Option<std::sync::Arc<dyn iyakki_shared::gitconn::GitHttp>>) -> crate::apps::git_wizard::WizEnv;
    /// Start the Quick install of Gitea (administrators); the sentence to show.
    fn quick_install_gitea(&self) -> Result<String, String>;
}

impl GitPort for AppCx {
    fn git_keys(&self) -> iyakki_shared::gitkeys::GitKeys {
        let audit = self.audit.clone();
        let user = self.session.user.clone();
        self.services.git_keys(&self.session.user, std::sync::Arc::new(move |a, d| audit.record(&user, a, d)), iyakki_shared::gitkeys::shared_session())
    }
    fn may_read(&self, path: &str) -> bool {
        self.session.policy.allows_read(&self.session.user, path)
    }
    fn may_reach(&self, host: &str) -> bool {
        self.session.policy.allows_host(host)
    }
    fn wizard_env(&self, keys: std::sync::Arc<iyakki_shared::gitkeys::GitKeys>, http: Option<std::sync::Arc<dyn iyakki_shared::gitconn::GitHttp>>) -> crate::apps::git_wizard::WizEnv {
        crate::apps::git_wizard::WizEnv::real(self, keys, http)
    }
    fn quick_install_gitea(&self) -> Result<String, String> {
        crate::apps::git_wizard::start_quick_install_gitea(self)
    }
}

//@TRAITS

// ---- the fake --------------------------------------------------------------------------------------------------------------------------

/// The fake port of this lane: every trait above answers from a field, and what a page asked is recorded. The data folder is the test's own.
#[cfg(test)]
pub(crate) struct FakeDevicePort {
    pub admin: std::sync::atomic::AtomicBool,
    pub user_name: String,
    pub dir: PathBuf,
    pub audit_lines: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    pub clock: iyakki_testkit::ManualClock,
    // Accessibility
    pub voice: std::sync::atomic::AtomicBool,
    pub speak_error: std::sync::Mutex<Option<String>>,
    pub spoken: std::sync::Mutex<Vec<String>>,
    pub braille: std::sync::Mutex<String>,
    // Displays
    pub monitors: std::sync::Mutex<Vec<iyakki_core_api::MonitorInfo>>,
    pub monitor_error: std::sync::Mutex<Option<String>>,
    pub mem: std::sync::Mutex<crate::memgate::MemFacts>,
    // System sounds
    pub sysnd: std::sync::Mutex<iyakki_shared::syssounds::SystemSounds>,
    pub sysnd_refuses: std::sync::Mutex<Option<String>>,
    pub muted: std::sync::atomic::AtomicBool,
    pub previewed: std::sync::Arc<std::sync::Mutex<Vec<iyakki_core_api::soundtypes::Cue>>>,
    // Home layout
    pub manage_users: std::sync::atomic::AtomicBool,
    pub tiles: std::sync::Mutex<Vec<iyakki_shared::store_tilehub::TileEntry>>,
    // the settings file
    pub settings: std::sync::Mutex<iyakki_shared::settings::Settings>,
    pub busy: std::sync::atomic::AtomicBool,
    // Screen saver
    pub guest: std::sync::atomic::AtomicBool,
    // Appearance
    pub pinnable_apps: std::sync::Mutex<Vec<(String, String)>>,
    pub sdk_data: std::sync::Mutex<Option<String>>,
    // Sound
    pub sound: std::sync::Mutex<iyakki_shared::audioout::SoundSettings>,
    pub sound_refuses: std::sync::Mutex<Option<String>>,
    pub voice_cfg: std::sync::Mutex<iyakki_shared::voice::VoiceSettings>,
    pub server_running: std::sync::atomic::AtomicBool,
    pub bt_view: std::sync::Mutex<iyakki_agent_api::BtSoundView>,
    pub bt_calls: std::sync::Mutex<Vec<String>>,
    pub t2: std::sync::Mutex<iyakki_shared::voicedesk::T2Mic>,
    pub mics: std::sync::Mutex<Vec<iyakki_shared::voicedesk::Mic>>,
    /// What the jobs the page starts did, in order (they run in the test, never on a thread of their own).
    pub sound_jobs: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// The loudness peak the microphone test hears.
    pub mic_peak: std::sync::atomic::AtomicI32,
    // Keyboard
    pub apple: std::sync::Mutex<iyakki_shared::keyboard::AppleState>,
    pub panels: std::sync::Mutex<Vec<String>>,
    pub touch_level: std::sync::atomic::AtomicU8,
    pub sound_out: std::sync::atomic::AtomicBool,
    // USB-C
    pub usb_env: std::sync::Mutex<Option<std::sync::Arc<iyakki_shared::directlink::fake::FakeEnv>>>,
    // Developer mode
    pub dev: std::sync::Arc<FakeDevMode>,
    pub ui_socket: std::sync::atomic::AtomicBool,
    pub resume_switch: std::sync::atomic::AtomicBool,
    pub addresses: std::sync::Mutex<Vec<(String, String)>>,
    // Dev cluster
    pub cluster: std::sync::Arc<FakeCluster>,
    pub programs: std::sync::Mutex<Vec<String>>,
    pub vault_names: std::sync::Arc<FakeVault>,
    // Git
    pub git_store: std::sync::Arc<iyakki_shared::gitkeys::MemStore>,
    pub git_session: std::sync::Arc<iyakki_shared::gitkeys::Session>,
    pub git_http: std::sync::Arc<iyakki_git_model::gitconn::FakeGitHttp>,
    pub gitea_install: std::sync::Mutex<Result<String, String>>,
    pub hosts_blocked: std::sync::atomic::AtomicBool,
    //@FIELDS
}

#[cfg(test)]
impl FakeDevicePort {
    pub(crate) fn new(admin: bool, tag: &str) -> FakeDevicePort {
        FakeDevicePort {
            admin: admin.into(),
            user_name: "amy".into(),
            dir: super::tests::temp_dir(&format!("fdp-{tag}")),
            audit_lines: Default::default(),
            clock: iyakki_testkit::ManualClock::new(1_000_000),
            voice: true.into(),
            speak_error: Default::default(),
            spoken: Default::default(),
            braille: std::sync::Mutex::new("No braille display found.".into()),
            monitors: Default::default(),
            monitor_error: Default::default(),
            mem: Default::default(),
            sysnd: Default::default(),
            sysnd_refuses: Default::default(),
            muted: false.into(),
            previewed: Default::default(),
            manage_users: false.into(),
            tiles: Default::default(),
            settings: Default::default(),
            busy: false.into(),
            guest: false.into(),
            pinnable_apps: std::sync::Mutex::new(vec![("chat".into(), "CHAT".into()), ("browser".into(), "BROWSER".into()), ("radio".into(), "RADIO".into()), ("calendar".into(), "CALENDAR".into())]),
            sdk_data: Default::default(),
            sound: Default::default(),
            sound_refuses: Default::default(),
            voice_cfg: Default::default(),
            server_running: false.into(),
            bt_view: Default::default(),
            bt_calls: Default::default(),
            t2: std::sync::Mutex::new(iyakki_shared::voicedesk::T2Mic::Absent),
            mics: Default::default(),
            sound_jobs: Default::default(),
            mic_peak: 2000.into(),
            apple: Default::default(),
            panels: Default::default(),
            touch_level: 1.into(),
            sound_out: true.into(),
            usb_env: Default::default(),
            dev: std::sync::Arc::new(FakeDevMode::default()),
            ui_socket: false.into(),
            resume_switch: false.into(),
            addresses: Default::default(),
            cluster: std::sync::Arc::new(FakeCluster::new(&std::env::temp_dir().join(format!("iyakki-fdp-cluster-{tag}-{}", std::process::id())))),
            programs: Default::default(),
            vault_names: Default::default(),
            git_store: Default::default(),
            git_session: Default::default(),
            git_http: Default::default(),
            gitea_install: std::sync::Mutex::new(Ok("Started the Quick install for Gitea.".into())),
            hosts_blocked: false.into(),
            //@INIT
        }
    }
    pub(crate) fn audited(&self) -> Vec<(String, String)> {
        self.audit_lines.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl PagePort for FakeDevicePort {
    fn is_admin(&self) -> bool {
        self.admin.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit_lines.lock().unwrap().push((action.to_string(), subject.to_string()));
    }
    fn data_dir(&self) -> PathBuf {
        self.dir.clone()
    }
    fn now_secs(&self) -> u64 {
        self.clock.now_secs()
    }
}

#[cfg(test)]
impl UserPort for FakeDevicePort {
    fn user(&self) -> String {
        self.user_name.clone()
    }
}

#[cfg(test)]
impl AccessPort for FakeDevicePort {
    fn voice_ready(&self) -> bool {
        self.voice.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn speak_test(&self, text: &str) -> Result<(), String> {
        if let Some(e) = self.speak_error.lock().unwrap().clone() {
            return Err(e);
        }
        self.spoken.lock().unwrap().push(text.to_string());
        Ok(())
    }
    fn braille_status(&self) -> String {
        self.braille.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl DisplaysPort for FakeDevicePort {
    fn monitors_list(&self) -> Vec<iyakki_core_api::MonitorInfo> {
        self.monitors.lock().unwrap().clone()
    }
    fn monitor_choose(&self, id: Option<&str>) -> Result<(), String> {
        if let Some(e) = self.monitor_error.lock().unwrap().clone() {
            return Err(e);
        }
        let mut m = self.monitors.lock().unwrap();
        if let Some(i) = id {
            if !m.iter().any(|x| x.id.as_deref() == Some(i)) {
                return Err("That screen does not answer now.".into());
            }
        }
        for x in m.iter_mut() {
            x.chosen = id.is_some() && x.id.as_deref() == id;
        }
        Ok(())
    }
    fn monitor_brightness(&self, percent: u8) -> Result<u8, String> {
        if let Some(e) = self.monitor_error.lock().unwrap().clone() {
            return Err(e);
        }
        let mut m = self.monitors.lock().unwrap();
        let Some(x) = m.iter_mut().find(|x| x.chosen) else { return Err("No screen is chosen.".into()) };
        x.percent = Ok(percent);
        Ok(percent)
    }
    fn mem_facts(&self) -> crate::memgate::MemFacts {
        *self.mem.lock().unwrap()
    }
}
#[cfg(test)]
impl SystemSoundsPort for FakeDevicePort {
    fn system_sounds(&self) -> iyakki_shared::syssounds::SystemSounds {
        self.sysnd.lock().unwrap().clone()
    }
    fn update_system_sounds(&self, f: &mut dyn FnMut(&mut iyakki_shared::syssounds::SystemSounds)) -> Result<iyakki_shared::syssounds::SystemSounds, String> {
        if let Some(e) = self.sysnd_refuses.lock().unwrap().clone() {
            return Err(e);
        }
        let mut cur = self.sysnd.lock().unwrap();
        f(&mut cur);
        Ok(cur.clone())
    }
    fn sound_muted(&self) -> bool {
        self.muted.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn preview_job(&self, cue: iyakki_core_api::soundtypes::Cue) -> Box<dyn FnOnce() -> Result<String, String> + Send> {
        let log = self.previewed.clone();
        Box::new(move || {
            log.lock().unwrap().push(cue);
            Ok(format!("Played the {} cue.", cue.id()))
        })
    }
}
#[cfg(test)]
impl HomeLayoutPort for FakeDevicePort {
    fn can_manage_users(&self) -> bool {
        self.manage_users.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn store_tiles(&self) -> Vec<iyakki_shared::store_tilehub::TileEntry> {
        self.tiles.lock().unwrap().clone()
    }
}
#[cfg(test)]
impl SettingsDataPort for FakeDevicePort {
    fn settings_get(&self) -> iyakki_shared::settings::Settings {
        self.settings.lock().unwrap().clone()
    }
    fn settings_update(&self, f: &mut dyn FnMut(&mut iyakki_shared::settings::Settings)) -> Result<(), SettingsWriteError> {
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
impl ScreenSaverPort for FakeDevicePort {
    fn is_guest(&self) -> bool {
        self.guest.load(std::sync::atomic::Ordering::SeqCst)
    }
}
#[cfg(test)]
impl AppearancePort for FakeDevicePort {
    fn pinnable(&self) -> Vec<(String, String)> {
        self.pinnable_apps.lock().unwrap().clone()
    }
    fn sdk_app_chosen(&self, app: &str) -> bool {
        iyakki_shared::sdkapps::is_chosen(self.sdk_data.lock().unwrap().as_deref(), app)
    }
    fn sdk_app_set(&self, app: &str, on: bool) -> Result<String, String> {
        let mut d = self.sdk_data.lock().unwrap();
        let next = iyakki_shared::sdkapps::with(d.as_deref(), app, on)?;
        *d = Some(next.clone());
        Ok(iyakki_shared::sdkapps::describe(Some(&next)))
    }
}
#[cfg(test)]
impl SoundPort for FakeDevicePort {
    fn sound_settings(&self) -> iyakki_shared::audioout::SoundSettings {
        self.sound.lock().unwrap().clone()
    }
    fn refresh_starting(&self) -> Option<String> {
        None
    }
    fn update_sound(&self, f: &mut dyn FnMut(&mut iyakki_shared::audioout::SoundSettings)) -> Result<iyakki_shared::audioout::SoundSettings, String> {
        if let Some(e) = self.sound_refuses.lock().unwrap().clone() {
            return Err(e);
        }
        let mut cur = self.sound.lock().unwrap();
        f(&mut cur);
        Ok(cur.clone())
    }
    fn voice_settings(&self) -> iyakki_shared::voice::VoiceSettings {
        self.voice_cfg.lock().unwrap().clone()
    }
    fn set_voice_mic(&self, id: &str) -> Result<(), String> {
        if let Some(e) = self.sound_refuses.lock().unwrap().clone() {
            return Err(e);
        }
        self.voice_cfg.lock().unwrap().mic = id.to_string();
        Ok(())
    }
    fn sound_server_running(&self) -> bool {
        self.server_running.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn test_sound_job(&self, device: Option<String>) -> SoundJob {
        let log = self.sound_jobs.clone();
        Box::new(move || {
            log.lock().unwrap().push(format!("test sound on {}", device.as_deref().unwrap_or("default")));
            Ok("Played a test sound.".to_string())
        })
    }
    fn test_pattern_job(&self, walk: bool) -> SoundJob {
        let log = self.sound_jobs.clone();
        Box::new(move || {
            log.lock().unwrap().push(format!("pattern walk={walk}"));
            Ok("Played the pattern.".to_string())
        })
    }
    fn hw_volume_check_job(&self, card: u32, device: u32, control: String) -> SoundJob {
        let log = self.sound_jobs.clone();
        Box::new(move || {
            log.lock().unwrap().push(format!("hw check {card},{device} {control}"));
            Ok("Played two tones.".to_string())
        })
    }
    fn bluetooth_sound_view(&self) -> iyakki_agent_api::BtSoundView {
        self.bt_view.lock().unwrap().clone()
    }
    fn bluetooth_sound_start(&self, address: &str) -> Result<String, String> {
        self.bt_calls.lock().unwrap().push(format!("start {address}"));
        self.bt_view.lock().unwrap().active = true;
        Ok("Sending the computer's sound to the headphones.".into())
    }
    fn bluetooth_sound_stop(&self) -> Result<String, String> {
        self.bt_calls.lock().unwrap().push("stop".into());
        self.bt_view.lock().unwrap().active = false;
        Ok("Sound plays on the speakers again.".into())
    }
    fn t2_mic(&self) -> iyakki_shared::voicedesk::T2Mic {
        *self.t2.lock().unwrap()
    }
    fn set_t2_capture(&self, on: bool) -> Result<(), String> {
        let mut t = self.t2.lock().unwrap();
        if *t == iyakki_shared::voicedesk::T2Mic::Absent {
            return Err("This computer has no T2 sound chip.".into());
        }
        *t = if on { iyakki_shared::voicedesk::T2Mic::On } else { iyakki_shared::voicedesk::T2Mic::Off };
        Ok(())
    }
    fn list_mics(&self) -> Vec<iyakki_shared::voicedesk::Mic> {
        self.mics.lock().unwrap().clone()
    }
    fn record_cmd_for(&self, base: &str, mic: Option<&iyakki_shared::voicedesk::Mic>) -> String {
        format!("{base} -D {}", mic.map(|m| m.id.as_str()).unwrap_or("default"))
    }
    fn mic_test_job(&self, label: String, cmd: String) -> SoundJob {
        let log = self.sound_jobs.clone();
        let peak = self.mic_peak.load(std::sync::atomic::Ordering::SeqCst);
        Box::new(move || {
            log.lock().unwrap().push(format!("mic test {cmd}"));
            if peak < 150 {
                Err(format!("{label} opened but it is silent (peak {peak})."))
            } else {
                Ok(format!("{label} works: peak level {peak} of 32767."))
            }
        })
    }
}
#[cfg(test)]
impl KeyboardPort for FakeDevicePort {
    fn apple_state(&self, _sys: &std::path::Path) -> iyakki_shared::keyboard::AppleState {
        self.apple.lock().unwrap().clone()
    }
    fn touch_panel_names(&self, _sys: &std::path::Path) -> Vec<String> {
        self.panels.lock().unwrap().clone()
    }
    fn touch_recover_status(&self) -> String {
        "idle".into()
    }
    fn touch_next_level(&self) -> u8 {
        self.touch_level.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn touch_probe_job(&self, _sys: std::path::PathBuf, level: u8) -> Box<dyn FnOnce() -> String + Send> {
        let log = self.sound_jobs.clone();
        Box::new(move || {
            log.lock().unwrap().push(format!("touch probe {level}"));
            format!("Probed at step {level}.")
        })
    }
    fn touch_reset_job(&self) -> Box<dyn FnOnce() -> String + Send> {
        let log = self.sound_jobs.clone();
        Box::new(move || {
            log.lock().unwrap().push("touch reset".into());
            "Reset the touch screen.".to_string()
        })
    }
    fn sound_output_present(&self) -> bool {
        self.sound_out.load(std::sync::atomic::Ordering::SeqCst)
    }
}
#[cfg(test)]
impl UsbcPort for FakeDevicePort {
    fn direct_link_env(&self) -> Option<std::sync::Arc<dyn iyakki_shared::directlink::Env>> {
        self.usb_env.lock().unwrap().clone().map(|e| e as std::sync::Arc<dyn iyakki_shared::directlink::Env>)
    }
}
/// A made-up developer mode: it keeps the on/off state, the switches and the paired computers in memory, records what was called, and
/// answers every other call with the trait's own "unavailable" default.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct FakeDevMode {
    pub on: std::sync::atomic::AtomicBool,
    pub core_on: std::sync::atomic::AtomicBool,
    pub relay_on: std::sync::atomic::AtomicBool,
    pub relay_address: std::sync::Mutex<String>,
    pub keys: std::sync::Mutex<Vec<iyakki_shared::devmode_types::DevKey>>,
    pub calls: std::sync::Mutex<Vec<String>>,
    pub refuse: std::sync::Mutex<Option<String>>,
}

#[cfg(test)]
impl FakeDevMode {
    fn note(&self, what: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(what.to_string());
        match self.refuse.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
    pub(crate) fn called(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl iyakki_agent_api::DevPairing for FakeDevMode {
    fn keys(&self) -> Vec<iyakki_shared::devmode_types::DevKey> {
        self.keys.lock().unwrap().clone()
    }
    fn is_on(&self) -> bool {
        self.on.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn core_enabled(&self) -> bool {
        self.core_on.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn set_core_enabled(&self, _policy: &iyakki_sec::policy::Policy, _user: &str, on: bool) -> Result<(), String> {
        self.note(&format!("core {on}"))?;
        self.core_on.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn disable(&self, _policy: &iyakki_sec::policy::Policy, _user: &str) -> Result<(), String> {
        self.note("disable")?;
        self.on.store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
impl iyakki_agent_api::DevTrust for FakeDevMode {}

#[cfg(test)]
impl iyakki_agent_api::DevRelay for FakeDevMode {
    fn relay_enabled(&self) -> bool {
        self.relay_on.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn set_relay_enabled(&self, _policy: &iyakki_sec::policy::Policy, _user: &str, on: bool) -> Result<(), String> {
        self.note(&format!("relay {on}"))?;
        self.relay_on.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn relay_url(&self) -> String {
        self.relay_address.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl DeveloperPort for FakeDevicePort {
    fn policy(&self) -> iyakki_sec::policy::Policy {
        if self.is_admin() { iyakki_sec::policy::Role::Admin.template() } else { iyakki_sec::policy::Role::User.template() }
    }
    fn devmode(&self) -> std::sync::Arc<dyn iyakki_agent_api::DevModeApi> {
        self.dev.clone()
    }
    fn wasm_tiles_enabled(&self) -> bool {
        self.settings.lock().unwrap().wasm_tiles
    }
    fn ui_socket_enabled(&self) -> bool {
        self.ui_socket.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn set_ui_socket(&self, on: bool) -> Result<String, String> {
        self.ui_socket.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(if on { "on".into() } else { "off".into() })
    }
    fn resume_switch_on(&self) -> bool {
        self.resume_switch.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn set_resume_switch(&self, on: bool) -> Result<(), String> {
        self.resume_switch.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn ipv4_addresses(&self) -> Vec<(String, String)> {
        self.addresses.lock().unwrap().clone()
    }
}
/// A made-up cluster supervisor: it keeps the installed state in memory and records what was asked of it.
#[cfg(test)]
pub(crate) struct FakeCluster {
    layout: iyakki_shared::k3sdev::Layout,
    pub installed: std::sync::Mutex<Option<iyakki_shared::k3sdev::State>>,
    pub autostart: std::sync::atomic::AtomicBool,
    pub fixed_ip: std::sync::atomic::AtomicBool,
    pub offline: std::sync::atomic::AtomicBool,
    pub calls: std::sync::Mutex<Vec<String>>,
    pub refuse: std::sync::Mutex<Option<String>>,
    pub log: std::sync::Mutex<String>,
}

#[cfg(test)]
impl FakeCluster {
    pub(crate) fn new(dir: &std::path::Path) -> FakeCluster {
        FakeCluster {
            layout: iyakki_shared::k3sdev::Layout::new(dir),
            installed: Default::default(),
            autostart: false.into(),
            fixed_ip: false.into(),
            offline: false.into(),
            calls: Default::default(),
            refuse: Default::default(),
            log: Default::default(),
        }
    }
    fn note(&self, what: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(what.to_string());
        match self.refuse.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
    pub(crate) fn called(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl iyakki_agent_api::DevCluster for FakeCluster {
    fn layout(&self) -> &iyakki_shared::k3sdev::Layout {
        &self.layout
    }
    fn installed(&self) -> Option<iyakki_shared::k3sdev::State> {
        self.installed.lock().unwrap().clone()
    }
    fn state(&self) -> iyakki_shared::k3sdev::Svc {
        iyakki_shared::k3sdev::Svc::Stopped
    }
    fn health(&self) -> iyakki_shared::k3sdev::Health {
        iyakki_shared::k3sdev::Health::default()
    }
    fn install(&self, _plan: &iyakki_shared::k3sdev::Plan, _st: &dyn iyakki_proxy::Streamer, _user: &str) -> Result<(), String> {
        self.note("install")
    }
    fn uninstall(&self, _typed: &str, _user: &str) -> Result<(), String> {
        self.note("uninstall")
    }
    fn stop(&self, _user: &str) -> Result<(), String> {
        self.note("stop")
    }
    fn start(self: std::sync::Arc<Self>, _user: &str) -> Result<(), String> {
        self.note("start")
    }
    fn restart(self: std::sync::Arc<Self>, _user: &str) -> Result<(), String> {
        self.note("restart")
    }
    fn upgrade(self: std::sync::Arc<Self>, _plan: &iyakki_shared::k3sdev::Plan, _st: &dyn iyakki_proxy::Streamer, _user: &str) -> Result<(), String> {
        self.note("upgrade")
    }
    fn upgrade_plan(&self, _st: &dyn iyakki_proxy::Streamer) -> Result<Option<iyakki_shared::k3sdev::Plan>, String> {
        Ok(None)
    }
    fn reset(&self, _typed: &str, _user: &str) -> Result<(), String> {
        self.note("reset")
    }
    fn refresh_health(&self) -> iyakki_shared::k3sdev::Health {
        iyakki_shared::k3sdev::Health::default()
    }
    fn situation(&self) -> iyakki_shared::k3sdev::Situation {
        iyakki_shared::k3sdev::Situation::Free { problems: Vec::new() }
    }
    fn describe(&self, _plan: &iyakki_shared::k3sdev::Plan) -> String {
        "a made-up plan".into()
    }
    fn log_tail(&self, _lines: usize) -> String {
        self.log.lock().unwrap().clone()
    }
    fn take_alerts(&self) -> Vec<iyakki_shared::k3sdev::Alert> {
        Vec::new()
    }
    fn set_audit(&self, _f: std::sync::Arc<dyn Fn(&str, &str, &str) + Send + Sync>) {}
    fn set_fixed_node_ip(&self, on: bool, _user: &str) -> Result<(), String> {
        self.note(&format!("fixed ip {on}"))?;
        self.fixed_ip.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn fixed_node_ip_enabled(&self) -> bool {
        self.fixed_ip.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn set_autostart(&self, on: bool, _user: &str) -> Result<(), String> {
        self.note(&format!("autostart {on}"))?;
        self.autostart.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn set_offline_on_purpose(&self, on: bool, _user: &str) -> Result<(), String> {
        self.note(&format!("offline {on}"))?;
        self.offline.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn offline_on_purpose(&self) -> bool {
        self.offline.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn kubectl(&self, _args: &[&str]) -> iyakki_shared::wsexec::Out {
        iyakki_shared::wsexec::Out::default()
    }
    fn modelhost_kube(self: std::sync::Arc<Self>) -> std::sync::Arc<dyn iyakki_shared::modelhost::Kube + Send + Sync> {
        self
    }
    fn kubectl_reader(self: std::sync::Arc<Self>) -> std::sync::Arc<dyn iyakki_shared::kubectl::Kubectl> {
        self
    }
    fn kubectl_cap(&self, _args: &[&str], _cap: usize) -> iyakki_shared::wsexec::Out {
        iyakki_shared::wsexec::Out::default()
    }
}

#[cfg(test)]
impl iyakki_shared::kubectl::Kubectl for FakeCluster {
    fn kubectl(&self, _args: &[&str]) -> Result<String, String> {
        Ok(String::new())
    }
}

#[cfg(test)]
impl iyakki_shared::modelhost::Kube for FakeCluster {
    fn kubectl(&self, _args: &[&str]) -> iyakki_shared::wsexec::Out {
        iyakki_shared::wsexec::Out::default()
    }
}

/// A made-up vault: the names of the system and per-person entries, in memory.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct FakeVault {
    pub system: std::sync::Mutex<Vec<String>>,
    pub user: std::sync::Mutex<Vec<(String, String)>>,
}

#[cfg(test)]
impl iyakki_agent_api::VaultRead for FakeVault {
    fn system_names(&self) -> Result<Vec<String>, iyakki_shared::vault::VaultError> {
        Ok(self.system.lock().unwrap().clone())
    }
    fn system_set(&self, name: &str, _value: &str) -> Result<(), iyakki_shared::vault::VaultError> {
        self.system.lock().unwrap().push(name.to_string());
        Ok(())
    }
    fn system_delete(&self, name: &str) -> Result<(), iyakki_shared::vault::VaultError> {
        self.system.lock().unwrap().retain(|n| n != name);
        Ok(())
    }
    fn user_names(&self, user: &str) -> Result<Vec<String>, iyakki_shared::vault::VaultError> {
        Ok(self.user.lock().unwrap().iter().filter(|(u, _)| u == user).map(|(_, n)| n.clone()).collect())
    }
    fn user_set(&self, user: &str, name: &str, _value: &str) -> Result<(), iyakki_shared::vault::VaultError> {
        self.user.lock().unwrap().push((user.to_string(), name.to_string()));
        Ok(())
    }
    fn user_get(&self, _user: &str, _name: &str) -> Result<zeroize::Zeroizing<String>, iyakki_shared::vault::VaultError> {
        Ok(zeroize::Zeroizing::new(String::new()))
    }
    fn user_delete(&self, user: &str, name: &str) -> Result<(), iyakki_shared::vault::VaultError> {
        self.user.lock().unwrap().retain(|(u, n)| !(u == user && n == name));
        Ok(())
    }
    fn user_vault_unlocked(&self, _user: &str) -> bool {
        true
    }
    fn reset_user_vault(&self, _user: &str) -> Result<(), String> {
        Ok(())
    }
    fn answer_secret_request(&self, _user: &str, _id: &str, _name: &str, _value: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn cancel_secret_request(&self, _user: &str, _id: &str) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
impl DevClusterPort for FakeDevicePort {
    fn k3sdev(&self) -> std::sync::Arc<dyn iyakki_agent_api::DevCluster> {
        self.cluster.clone()
    }
    fn vault(&self) -> std::sync::Arc<dyn iyakki_agent_api::VaultRead> {
        self.vault_names.clone()
    }
    fn streamer(&self) -> Option<std::sync::Arc<dyn iyakki_proxy::Streamer>> {
        None
    }
    fn role(&self) -> iyakki_sec::policy::Role {
        if self.is_admin() { iyakki_sec::policy::Role::Admin } else { iyakki_sec::policy::Role::User }
    }
    fn allows_read(&self, _path: &str) -> bool {
        true
    }
    fn on_path(&self, program: &str) -> bool {
        self.programs.lock().unwrap().iter().any(|p| p == program)
    }
    fn b5(&self) -> std::sync::Arc<dyn iyakki_agent_api::ServicesFacetB5> {
        // the fake has no services: the one call that asks for them (the setup lookup) is not exercised on the fake
        std::sync::Arc::new(iyakki_agent_api::NoB5)
    }
    fn audit_sink(&self) -> std::sync::Arc<dyn Fn(&str, &str, &str) + Send + Sync> {
        std::sync::Arc::new(|_, _, _| {})
    }
    fn platform(&self, _run: std::sync::Arc<dyn iyakki_shared::wsexec::Runner>, _env: iyakki_shared::devcluster::Env) -> Option<(std::sync::Arc<dyn iyakki_agent_api::PlatformApi>, iyakki_agent_api::PlatformHandles)> {
        None
    }
    fn fleet(&self) -> Option<std::sync::Arc<dyn iyakki_agent_api::FleetApi>> {
        None
    }
}
#[cfg(test)]
impl GitPort for FakeDevicePort {
    fn git_keys(&self) -> iyakki_shared::gitkeys::GitKeys {
        let lines = self.audit_lines.clone();
        let user = self.user_name.clone();
        let _ = user;
        iyakki_shared::gitkeys::GitKeys::new(&self.dir.join("ssh").join(&self.user_name), self.git_store.clone(), std::sync::Arc::new(move |a, d| lines.lock().unwrap().push((a.to_string(), d.to_string()))), self.git_session.clone())
    }
    fn may_read(&self, path: &str) -> bool {
        path.starts_with(self.dir.to_str().unwrap_or("/nonexistent"))
    }
    fn may_reach(&self, _host: &str) -> bool {
        !self.hosts_blocked.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn wizard_env(&self, keys: std::sync::Arc<iyakki_shared::gitkeys::GitKeys>, http: Option<std::sync::Arc<dyn iyakki_shared::gitconn::GitHttp>>) -> crate::apps::git_wizard::WizEnv {
        let blocked = self.hosts_blocked.load(std::sync::atomic::Ordering::SeqCst);
        crate::apps::git_wizard::WizEnv {
            keys,
            http: http.unwrap_or_else(|| self.git_http.clone()),
            dir: self.dir.clone(),
            user: self.user_name.clone(),
            egress: std::sync::Arc::new(move |h| if blocked { Err(format!("Your account may not reach {h}.")) } else { Ok(()) }),
            kube: std::sync::Arc::new(|_| iyakki_shared::pvcguard::KOut::default()),
        }
    }
    fn quick_install_gitea(&self) -> Result<String, String> {
        self.gitea_install.lock().unwrap().clone()
    }
}
//@FAKEIMPLS
