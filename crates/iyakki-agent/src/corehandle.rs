//! The real `iyakki_core_api::Core` for the program that holds the agent (UI program step, alpha.5 item 0a). The UI never names the agent for
//! what is on this handle; it asks `cx.core`. Built once, in the shell's composition root.

use crate::mixer::{self, SourceKind};
use crate::services::Services;
use iyakki_core_api::{AppData, Desktop, FlowsView, HomeFiles, SessionResume, SettingsView, VaultView, UiSocket, AppStatus, AppStatuses, Core, Mixer, MixerStats, MonitorInfo, Monitors, SoundKind, StatusKind};
use iyakki_sec::policy::App as PApp;
use std::any::Any;
use std::sync::Arc;

pub struct AgentCore {
    services: Arc<Services>,
    ui_state: iyakki_core_local::uistate::LocalUiState,
    settings: Arc<crate::contract_local::CoreSettings>,
    /// The assistant an app embeds (the shell gives it the model, the accounts' limits and the audit log through `with_assistant`).
    assistant: Option<crate::embedassist::AgentAssistant>,
    vault: crate::contract_local::CoreVault,
    /// The camera views an app embeds (the shell gives it the hub's seams through `with_cameras`).
    cameras: Option<Arc<crate::embedcameras::AgentCameras>>,
    /// The events delivered to apps: the engine of the services plus the sources that are looked at when an app asks (`appevent_sources.rs`).
    app_events: Arc<crate::appevent_sources::SourcedEvents>,
    /// What an app's event handler may do (a note, a flow, a tool request that waits for the person).
    app_actions: crate::appactions_real::AgentActions,
    /// The media player an app embeds (the shell gives it the audit log through `with_media`).
    media: Option<crate::embedmedia::AgentMedia>,
    embedded: Arc<crate::embedbrowser::Embedded>,
    terminals: crate::terminalfacet::Terminals,
    file_book: iyakki_core_api::ChangeBook,
}

impl AgentCore {
    /// The embedded assistant: sessions that apps open, with the host's model and limits and the audit sink of the machine.
    pub fn with_assistant(mut self, host: Arc<dyn crate::embedassist::AssistantHost>, audit: Arc<dyn iyakki_core_api::AuditSink>) -> AgentCore {
        self.assistant = Some(crate::embedassist::AgentAssistant::new(host, audit));
        self
    }

    /// [`AgentCore::with_assistant`] with the device's host: the account's agent switch and budget come from `policy_of`, the model is `chat`.
    pub fn with_embedded_assistant(self, chat: Arc<dyn crate::chat::ChatBackend>, policy_of: Arc<dyn Fn(&str) -> Option<iyakki_sec::policy::Policy> + Send + Sync>, audit: Arc<dyn iyakki_core_api::AuditSink>) -> AgentCore {
        let host = Arc::new(crate::embedassist::ServicesHost { services: self.services.clone(), chat, policy_of });
        self.with_assistant(host, audit)
    }

    /// The embedded camera views of this machine, over the camera hub, the motion watcher and the recorder that the sign-in started
    /// (`embedcameras::from_running`): the same object for every handle over the same services. `policy_of` gives an account's policy.
    pub fn with_embedded_cameras(mut self, policy_of: Arc<dyn Fn(&str) -> Option<iyakki_sec::policy::Policy> + Send + Sync>) -> AgentCore {
        self.cameras = Some(crate::embedcameras::from_running(self.services.dir(), policy_of));
        self
    }

    /// The embedded media player over the device's home folder, mixer and radio list.
    pub fn with_media(mut self, audit: Arc<dyn iyakki_core_api::AuditSink>, policy_of: Arc<dyn Fn(&str) -> Option<iyakki_sec::policy::Policy> + Send + Sync>) -> AgentCore {
        let host = Arc::new(crate::embedmedia::ServicesMediaHost { services: self.services.clone(), policy_of });
        self.media = Some(crate::embedmedia::AgentMedia::new(host, audit));
        self
    }

    pub fn new(services: Arc<Services>) -> AgentCore {
        let settings = Arc::new(crate::contract_local::CoreSettings::new(services.settings.clone()));
        // the sources look on a clock of the core's own, so what happens while an app is closed waits for it
        let events = Arc::new(crate::appevent_sources::SourcedEvents::new(services.app_events.clone(), crate::appevent_sources::real_inputs(&services)));
        crate::appevent_sources::SourcedEvents::spawn_ticker(&events);
        AgentCore { app_actions: crate::appactions_real::AgentActions::new(services.clone()), app_events: events, cameras: None, media: None, embedded: crate::embedbrowser::for_dir(services.dir()), assistant: None, ui_state: iyakki_core_local::uistate::LocalUiState::new(services.dir()), settings, vault: crate::contract_local::CoreVault::new(services.clone()), terminals: crate::terminalfacet::Terminals::new(services.clone()), file_book: Default::default(), services }
    }

    /// The embedded browser as a shared handle (the web service's tile host serves a tile's `browser` calls through it).
    pub fn shared_browser(&self) -> Arc<dyn iyakki_core_api::BrowserSessions> {
        Arc::new(crate::embedbrowser::SharedSessions(self.embedded.clone()))
    }

    /// Tell the core where a person's policy comes from (the accounts), which the embedded terminal needs. Without it the terminal says it
    /// has no account to serve.
    pub fn with_policies(self, p: Arc<dyn iyakki_core_api::PolicySource>) -> AgentCore {
        self.terminals.set_policies(p);
        self
    }
}

fn kind(k: SoundKind) -> SourceKind {
    match k {
        SoundKind::Radio => SourceKind::Radio,
        SoundKind::Media => SourceKind::Media,
        SoundKind::Notification => SourceKind::Notification,
        SoundKind::Speech => SourceKind::Speech,
        SoundKind::Alarm => SourceKind::Alarm,
    }
}

impl Mixer for AgentCore {
    fn stats(&self) -> Option<MixerStats> {
        mixer::stats_if_enabled().map(|s| MixerStats { sink_open: s.sink_open, detail: s.detail(), last_error: s.last_error.clone() })
    }

    fn active_sources(&self) -> Vec<(String, u32)> {
        mixer::active_sources()
    }

    fn play_wav(&self, k: SoundKind, label: &str, wav: &[u8]) -> Result<Box<dyn Any + Send>, String> {
        let out = self.services.play_wav_output(kind(k), label, wav)?;
        Ok(Box::new(out))
    }
}

impl iyakki_core_api::SoundControl for AgentCore {
    fn state(&self) -> iyakki_core_api::SoundState {
        let g = crate::audioout::global();
        if !g.is_active() {
            return iyakki_core_api::SoundState::default();
        }
        let st = g.status();
        iyakki_core_api::SoundState {
            active: true,
            chosen: st.choice.output,
            reason: st.choice.reason,
            by_person: st.choice.chosen,
            sound_server: st.sound_server,
            mix_channels: st.mix.map(|m| m.channels),
            starting: st.starting,
            hw_ignored: st.hw_ignored,
            busy: st.busy,
            softvol_card: st.softvol,
        }
    }
    fn refresh_starting(&self) -> Option<String> {
        let g = crate::audioout::global();
        if g.is_active() { g.refresh().starting } else { None }
    }
    fn prefs(&self) -> iyakki_core_api::soundtypes::SoundSettings {
        crate::audioout::global().prefs()
    }
    fn outputs_at(&self, proc_root: &std::path::Path) -> Vec<iyakki_core_api::soundtypes::Output> {
        crate::audioout::outputs_at(proc_root)
    }
    fn card_starting(&self, proc_root: &std::path::Path, sys: &std::path::Path) -> Option<u32> {
        crate::audioout::card_starting(proc_root, sys)
    }
    fn volume_note(&self) -> Option<String> {
        crate::audioout::global().volume_note()
    }
    fn take_notices(&self) -> Vec<String> {
        crate::audioout::global().take_notices()
    }
    fn server_running(&self) -> bool {
        crate::audioout::sound_server_running()
    }
    fn command_with_device(&self, play_cmd: &str, device: &str) -> Result<String, String> {
        crate::audioout::command_with_device(play_cmd, device)
    }
}

impl iyakki_core_api::CuePlayer for AgentCore {
    fn out(&self) -> Arc<dyn iyakki_core_api::soundtypes::CueOut> {
        self.services.cue_out()
    }
    fn play(&self, cue: iyakki_core_api::soundtypes::Cue, out: Arc<dyn iyakki_core_api::soundtypes::CueOut>) -> iyakki_core_api::soundtypes::CueHandle {
        self.services.play_cue(cue, out)
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The statuses of three more apps from facts the core already keeps (F-LAUNCH-09): INBOX (unread entries that need the person, the
/// bell's number), CAMERAS (the cameras recording now) and K8S (the dev cluster runs). Pure, so a test gives it the facts.
pub(crate) fn more_statuses(inbox_attention: usize, recording: &[String], cluster_running: bool, flows_running: usize) -> Vec<(String, AppStatus)> {
    let mut v = Vec::new();
    if inbox_attention > 0 {
        let line = plural(inbox_attention, "message needs you", "messages need you");
        v.push((PApp::Inbox.id().into(), AppStatus { kind: StatusKind::Attention, line: line.clone(), count: inbox_attention, needs: Some(("Inbox".into(), line)) }));
    }
    if !recording.is_empty() {
        let line = if recording.len() == 1 { format!("Recording {}", recording[0]) } else { format!("Recording {} cameras", recording.len()) };
        v.push((PApp::Cameras.id().into(), AppStatus { kind: StatusKind::Running, line, count: 0, needs: None }));
    }
    if flows_running > 0 {
        v.push((PApp::Flows.id().into(), AppStatus { kind: StatusKind::Running, line: plural(flows_running, "flow running", "flows running"), count: 0, needs: None }));
    }
    if cluster_running {
        v.push((PApp::K8s.id().into(), AppStatus { kind: StatusKind::Running, line: "Dev cluster running".into(), count: 0, needs: None }));
    }
    v
}

impl AppStatuses for AgentCore {
    /// What the apps have to say, from the places the core already keeps it: the approvals the assistant waits on (CHAT), overdue tasks
    /// (CALENDAR), the update check (STORE), the boot drive (DISKS), the station that plays (RADIO), the song or video that plays (MEDIA)
    /// and the jobs that run (RUNS). An app that has nothing to say is not in the list.
    fn all(&self, user: &str) -> Vec<(String, AppStatus)> {
        let mut v: Vec<(String, AppStatus)> = Vec::new();
        let sv = &self.services;
        let approvals = sv.approvals(user).len() + sv.secret_requests.pending(user).len() + sv.browser.consent.pending(user).len();
        if approvals > 0 {
            v.push((PApp::Chat.id().into(), AppStatus { kind: StatusKind::Attention, line: format!("{} waiting for you", approvals), count: approvals, needs: Some(("Approvals waiting".into(), format!("{approvals} waiting for you"))) }));
        }
        let counts = sv.inbox.counts(user, crate::clock::now_unix());
        if counts.overdue > 0 {
            let line = plural(counts.overdue, "overdue task", "overdue tasks");
            v.push((PApp::Calendar.id().into(), AppStatus { kind: StatusKind::Attention, line: line.clone(), count: counts.overdue, needs: Some(("Overdue".into(), line)) }));
        }
        let updates = crate::store::updatecheck::Report::load(sv.dir()).map_or(0, |r| r.count());
        if updates > 0 {
            v.push((PApp::Store.id().into(), AppStatus { kind: StatusKind::Setup, line: plural(updates, "update", "updates"), count: updates, needs: Some(("Updates".into(), format!("{updates} waiting in STORE"))) }));
        }
        if !iyakki_storage::loss::snapshot().state.healthy() {
            v.push((PApp::Disks.id().into(), AppStatus { kind: StatusKind::Attention, line: "drive problem".into(), count: 0, needs: Some(("Boot drive".into(), "a drive problem was found: open SYSTEM".into())) }));
        }
        match sv.radio.state() {
            crate::radio::RadioState::Playing { station, .. } => v.push((PApp::Radio.id().into(), AppStatus { kind: StatusKind::Running, line: format!("Playing {station}"), count: 0, needs: None })),
            crate::radio::RadioState::Connecting(station) => v.push((PApp::Radio.id().into(), AppStatus { kind: StatusKind::Running, line: format!("Connecting to {station}"), count: 0, needs: None })),
            _ => {}
        }
        if let Some(label) = crate::media::shared().now_playing() {
            v.push((PApp::Media.id().into(), AppStatus { kind: StatusKind::Running, line: format!("Playing {label}"), count: 0, needs: None }));
        }
        // what browser tiles say about their own icons: only for an app that has nothing of its own to say above
        let said: Vec<(String, (Option<u32>, String))> = sv.tile_status.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().filter(|((u, _), _)| u == user).map(|((_, t), v)| (t.clone(), v.clone())).collect();
        for (tile, (badge, line)) in said {
            if !v.iter().any(|(id, _)| *id == tile) {
                v.push((tile, AppStatus { kind: StatusKind::Running, line, count: badge.unwrap_or(0) as usize, needs: None }));
            }
        }
        let running = sv.jobs.running_count(user);
        if running > 0 {
            v.push((PApp::Runs.id().into(), AppStatus { kind: StatusKind::Running, line: plural(running, "job running", "jobs running"), count: 0, needs: None }));
        }
        let waiting = crate::store::sdkv2_engines::viewer_waiting(&self.ui_state, user).len();
        if waiting > 0 {
            let line = plural(waiting, "document waiting", "documents waiting");
            v.push((PApp::Markdown.id().into(), AppStatus { kind: StatusKind::Attention, line: line.clone(), count: waiting, needs: Some(("Documents".into(), format!("{line} in MARKDOWN"))) }));
        }
        v.extend(more_statuses(counts.attention, &crate::camrec::active_names(sv.dir()), crate::k3sdev::managed_running(), crate::flows::running_for(user)));
        v
    }

    fn waiting_calls(&self, user: &str) -> Vec<iyakki_core_api::WaitingCall> {
        self.services.approvals(user).into_iter().map(|a| iyakki_core_api::WaitingCall { tool: a.tool, summary: a.summary }).collect()
    }

    fn tokens_24h(&self, user: &str) -> Option<(u64, u64, u64)> {
        let from = crate::clock::now_unix() - 86_400;
        let (mut tin, mut tout, mut calls) = (0u64, 0u64, 0u64);
        for t in self.services.usage.by_source(Some(user), from) {
            tin += t.input;
            tout += t.output;
            calls += t.calls;
        }
        (calls > 0).then_some((tin, tout, calls))
    }
}

/// The screen the person chose is kept in this file of the data folder (one id).
const CHOSEN_FILE: &str = "ddcci-chosen";

impl AgentCore {
    fn screens(&self) -> Vec<crate::ddcci_bus::Screen> {
        crate::ddcci_bus::list_real(std::path::Path::new("/sys"), std::path::Path::new("/dev"))
    }
    fn chosen_id(&self) -> Option<String> {
        std::fs::read_to_string(self.services.dir().join(CHOSEN_FILE)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }
}

impl Monitors for AgentCore {
    fn list(&self) -> Vec<MonitorInfo> {
        let chosen = self.chosen_id();
        self.screens().into_iter().map(|s| MonitorInfo { chosen: s.id.is_some() && s.id == chosen, id: s.id, label: s.label, connector: s.connector, percent: s.brightness }).collect()
    }
    fn choose(&self, id: Option<&str>) -> Result<(), String> {
        let file = self.services.dir().join(CHOSEN_FILE);
        match id {
            None => {
                let _ = std::fs::remove_file(&file);
                Ok(())
            }
            Some(i) => {
                if !self.screens().iter().any(|s| s.id.as_deref() == Some(i)) {
                    return Err("no screen with that id answers over DDC/CI now (monitor_brightness lists them)".into());
                }
                std::fs::write(&file, i).map_err(|e| format!("the choice could not be kept: {e}"))
            }
        }
    }
    fn set_brightness(&self, percent: u8) -> Result<u8, String> {
        let id = self.chosen_id().ok_or("no screen is chosen; choose one first")?;
        let buses = iyakki_core_local::i2c::ddc_buses(std::path::Path::new("/sys"));
        crate::ddcci_bus::set(&buses, &mut |n| crate::ddcci_bus::open_real(std::path::Path::new("/dev"), n), &crate::ddcci::Chosen(Some(id.clone())), &id, percent).map(|b| b.percent()).map_err(|e| e.to_string())
    }
}

impl AppData for AgentCore {
    fn get(&self, user: &str, app: &str, key: &str) -> Option<String> {
        self.ui_state.get(user, app, key)
    }
    fn set(&self, user: &str, app: &str, key: &str, value: &str) -> Result<(), String> {
        self.ui_state.set(user, app, key, value)
    }
    fn remove(&self, user: &str, app: &str, key: &str) -> Result<(), String> {
        self.ui_state.remove(user, app, key)
    }
    fn keys(&self, user: &str, app: &str) -> Vec<String> {
        self.ui_state.keys(user, app)
    }
}

impl Desktop for AgentCore {
    fn dock(&self, user: &str) -> Option<Vec<String>> {
        self.ui_state.dock(user)
    }
    fn set_dock(&self, user: &str, items: &[String]) -> Result<(), String> {
        self.ui_state.set_dock(user, items)
    }
    fn wallpaper(&self, user: &str) -> Option<String> {
        self.ui_state.wallpaper(user)
    }
    fn set_wallpaper(&self, user: &str, key: &str) -> Result<(), String> {
        self.ui_state.set_wallpaper(user, key)
    }
    fn screen_wallpaper(&self, user: &str, screen: u32) -> Option<String> {
        self.ui_state.screen_wallpaper(user, screen)
    }
    fn set_screen_wallpaper(&self, user: &str, screen: u32, key: &str) -> Result<(), String> {
        self.ui_state.set_screen_wallpaper(user, screen, key)
    }
    fn screen_dock(&self, user: &str, screen: u32) -> Option<String> {
        self.ui_state.screen_dock(user, screen)
    }
    fn set_screen_dock(&self, user: &str, screen: u32, position: &str) -> Result<(), String> {
        self.ui_state.set_screen_dock(user, screen, position)
    }
}

impl UiSocket for AgentCore {
    fn status(&self) -> String {
        crate::contract_local::uiserve::status_text(self.services.dir(), false)
    }
    fn enabled(&self) -> bool {
        crate::contract_local::uiserve::load(self.services.dir()).enabled
    }
    fn set_enabled(&self, on: bool) -> Result<String, String> {
        let mut c = crate::contract_local::uiserve::load(self.services.dir());
        c.enabled = on;
        crate::contract_local::uiserve::save(self.services.dir(), &c)?;
        Ok(UiSocket::status(self))
    }
}

/// The text of a setting value, the way a settings page would say it.
pub fn setting_text(v: &iyakki_core_api::contract::SettingValue) -> String {
    use iyakki_core_api::contract::SettingValue as V;
    match v {
        V::Bool(b) => b.to_string(),
        V::Int(i) => i.to_string(),
        V::Text(t) => t.clone(),
    }
}

impl HomeFiles for AgentCore {
    fn list_markdown(&self, user: &str) -> Vec<String> {
        iyakki_core_local::homefiles::list_markdown(&crate::scope::home_root_for_writes(user, self.services.dir()))
    }
    fn read_markdown(&self, user: &str, name: &str) -> Result<String, String> {
        iyakki_core_local::homefiles::read_markdown(&crate::scope::home_root_for_writes(user, self.services.dir()), name)
    }
}

impl iyakki_core_api::Files for AgentCore {
    fn list(&self, user: &str, dir: &str) -> Result<iyakki_core_api::Listing, String> {
        iyakki_core_local::homebrowse::list(&crate::scope::home_root_for_writes(user, self.services.dir()), dir)
    }
    fn read_text(&self, user: &str, path: &str, max: usize) -> Result<String, String> {
        iyakki_core_local::homebrowse::read_text(&crate::scope::home_root_for_writes(user, self.services.dir()), path, max)
    }
    fn book(&self) -> Option<&iyakki_core_api::ChangeBook> {
        Some(&self.file_book)
    }
    fn apply(&self, user: &str, c: &iyakki_core_api::FileChange) -> Result<String, String> {
        let dir = self.services.dir();
        let trash = dir.join("trash").join(user.chars().filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')).collect::<String>());
        iyakki_core_local::homebrowse::apply(&crate::scope::home_root_for_writes(user, dir), &trash, c)
    }
    fn change(&self, user: &str, c: &iyakki_core_api::FileChange) -> Result<iyakki_core_api::ChangeAsk, String> {
        iyakki_core_api::files::check_change(c)?;
        let ask = self.file_book.ask(user, c)?;
        // names only: the text of a save is never in the log
        self.services.browser.audit(user, "files.change.asked", &iyakki_core_api::files::describe(c));
        Ok(ask)
    }
    fn decide(&self, user: &str, call: &str, approve: bool) -> Result<String, String> {
        let c = self.file_book.take(user, call).ok_or("there is no such question waiting")?;
        let what = iyakki_core_api::files::describe(&c);
        if !approve {
            self.services.browser.audit(user, "files.change.denied", &what);
            return Ok("Not changed: you said no.".into());
        }
        self.services.browser.audit(user, "files.change.approved", &what);
        let r = iyakki_core_api::files::check_change(&c).and_then(|_| iyakki_core_api::Files::apply(self, user, &c));
        self.services.browser.audit(user, if r.is_ok() { "files.change.done" } else { "files.change.failed" }, &what);
        r
    }
}

impl FlowsView for AgentCore {
    fn list(&self, user: &str, may_use: bool) -> Result<Vec<(String, String)>, String> {
        if !may_use {
            return Err("your account may not use FLOWS".into());
        }
        let env = crate::flows::FlowEnv::existing(&self.services, user).ok_or("no flows engine is running for this person (it opens when they sign in)")?;
        Ok(env.summaries().into_iter().map(|s| (s.id, s.name)).collect())
    }
    fn scheduler_running(&self, user: &str) -> bool {
        crate::flows::FlowEnv::scheduler_running(&self.services, user)
    }
    fn last_flow_recorded(&self, user: &str) -> bool {
        crate::flows::last_flow_recorded(&self.services, user)
    }
    fn clear_last_flow(&self, user: &str) {
        crate::flows::clear_last_flow(&self.services, user);
    }
    fn stop_for_logout(&self, user: &str) {
        crate::flows::FlowEnv::stop_for_logout(&self.services, user);
    }
}

impl VaultView for AgentCore {
    fn status(&self, user: &str) -> Result<(bool, u32), String> {
        use iyakki_core_api::contract::{CallContext, Vault as _};
        self.vault.status(&CallContext::local(user, false)).map(|s| (s.locked, s.entries)).map_err(|e| e.to_string())
    }
}

impl SettingsView for AgentCore {
    fn keys(&self) -> Vec<String> {
        use iyakki_core_api::contract::{CallContext, Settings as _};
        let mut k: Vec<String> = self.settings.schema(&CallContext::local("ui", false)).map(|s| s.into_iter().map(|x| x.key).collect()).unwrap_or_default();
        k.sort();
        k
    }
    fn get(&self, key: &str) -> Result<String, String> {
        use iyakki_core_api::contract::{CallContext, Settings as _};
        self.settings.get(&CallContext::local("ui", false), key).map(|v| setting_text(&v)).map_err(|e| e.to_string())
    }
}

impl SessionResume for AgentCore {
    fn allowed(&self) -> bool {
        crate::devmode::is_on(self.services.dir()) && self.switch_on()
    }
    fn switch_on(&self) -> bool {
        iyakki_core_local::resume::Resume::new(self.services.dir()).switch_on()
    }
    fn set_switch(&self, on: bool) -> Result<(), String> {
        iyakki_core_local::resume::Resume::new(self.services.dir()).set_switch(on)
    }
    fn remember(&self, user: &str) {
        let r = iyakki_core_local::resume::Resume::new(self.services.dir());
        if self.allowed() {
            let _ = r.remember(user, crate::clock::now_unix() as u64);
        } else {
            r.forget();
        }
    }
    fn who(&self) -> Option<String> {
        if !self.allowed() {
            return None;
        }
        iyakki_core_local::resume::Resume::new(self.services.dir()).who(crate::clock::now_unix() as u64)
    }
    fn forget(&self) {
        iyakki_core_local::resume::Resume::new(self.services.dir()).forget();
    }
}

impl iyakki_core_api::CloudModes for AgentCore {
    fn detect(&self) -> iyakki_cloudmode::CloudMode {
        crate::cloudmode::detect(self.services.cloud_setting())
    }
}

/// The file scope of this computer (no services needed): what the UI asks through `iyakki_apps::file_scope()` and `Core::file_scope()`.
pub struct LocalFileScope;

impl iyakki_core_api::FileScope for LocalFileScope {
    fn check_read(&self, policy: &iyakki_sec::policy::Policy, user: &str, p: &std::path::Path) -> Result<(), String> {
        crate::scope::check_read(policy, user, p)
    }
    fn check_write(&self, policy: &iyakki_sec::policy::Policy, user: &str, p: &std::path::Path) -> Result<(), String> {
        crate::scope::check_write(policy, user, p)
    }
    fn home_root(&self, user: &str, data_dir: &std::path::Path) -> std::path::PathBuf {
        crate::scope::home_root(user, data_dir)
    }
    fn roots(&self, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path) -> iyakki_filescope::FileRoots {
        crate::scope::file_roots(policy, user, data_dir)
    }
    fn roots_with_home(&self, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path, home: std::path::PathBuf) -> iyakki_filescope::FileRoots {
        crate::scope::file_roots_with_home(policy, user, data_dir, home)
    }
    fn home_root_for_writes(&self, user: &str, data_dir: &std::path::Path) -> std::path::PathBuf {
        crate::scope::home_root_for_writes(user, data_dir)
    }
    fn system_home(&self, user: &str) -> Option<std::path::PathBuf> {
        crate::scope::system_home(user)
    }
    fn resolve(&self, cwd: &std::path::Path, input: &str) -> Result<std::path::PathBuf, String> {
        crate::scope::resolve(cwd, input)
    }
    fn resolve_for_write(&self, cwd: &std::path::Path, input: &str) -> Result<std::path::PathBuf, String> {
        crate::scope::resolve_for_write(cwd, input)
    }
    fn ensure_parent(&self, path: &std::path::Path) -> Result<(), String> {
        crate::scope::ensure_parent(path)
    }
    fn explain_io(&self, e: &std::io::Error, path: &std::path::Path, action: &str) -> String {
        crate::scope::explain_io(e, path, action)
    }
}

/// The machine facts of this computer (no services needed: `iyakki top` uses it too).
pub struct LocalHost;

impl iyakki_core_api::HostFacts for LocalHost {
    fn cmdline(&self) -> String {
        crate::sysinfo::cmdline(crate::sysinfo::proc_root())
    }
    fn memory(&self) -> iyakki_core_api::sysinfo::Memory {
        crate::sysinfo::memory(&crate::sysinfo::SysRoots::real())
    }
    fn product_name(&self) -> String {
        crate::sysinfo::product_name(crate::sysinfo::sys_root())
    }
    fn uptime_secs(&self) -> u64 {
        crate::sysinfo::uptime_now()
    }
    fn self_owner(&self) -> (u32, u32) {
        crate::sysinfo::self_owner(crate::sysinfo::proc_root())
    }
    fn shared_memory_dir(&self) -> std::path::PathBuf {
        crate::sysinfo::shared_memory_dir()
    }
    fn proc_root(&self) -> std::path::PathBuf {
        crate::sysinfo::proc_root().to_path_buf()
    }
    fn cpu_times(&self, r: &iyakki_core_api::sysinfo::SysRoots) -> iyakki_core_api::sysinfo::CpuTimes {
        crate::sysinfo::cpu_times(r)
    }
    fn cpu_count(&self, r: &iyakki_core_api::sysinfo::SysRoots) -> usize {
        crate::sysinfo::cpu_count(r)
    }
    fn memory_of(&self, r: &iyakki_core_api::sysinfo::SysRoots) -> iyakki_core_api::sysinfo::Memory {
        crate::sysinfo::memory(r)
    }
    fn load_average(&self, r: &iyakki_core_api::sysinfo::SysRoots) -> (f32, f32, f32) {
        crate::sysinfo::load_average(r)
    }
    fn uptime_of(&self, r: &iyakki_core_api::sysinfo::SysRoots) -> u64 {
        crate::sysinfo::uptime_secs(r)
    }
    fn hottest_temp(&self, r: &iyakki_core_api::sysinfo::SysRoots) -> Option<f32> {
        crate::sysinfo::hottest_temp(r)
    }
    fn net_counters(&self, r: &iyakki_core_api::sysinfo::SysRoots) -> Vec<iyakki_core_api::sysinfo::NetCounters> {
        crate::sysinfo::net_counters(r)
    }
    fn disk_counters(&self, r: &iyakki_core_api::sysinfo::SysRoots) -> Vec<iyakki_core_api::sysinfo::DiskCounters> {
        crate::sysinfo::disk_counters(r)
    }
    fn signal(&self, pid: u32, hard: bool) -> Result<(), String> {
        crate::sysinfo::signal(pid, hard)
    }
    fn processes(&self, r: &iyakki_core_api::sysinfo::SysRoots) -> Vec<iyakki_core_api::sysinfo::Proc> {
        crate::sysinfo::processes(r)
    }
    fn net_use(&self, r: &iyakki_core_api::sysinfo::SysRoots, pids: &[u32]) -> std::collections::HashMap<u32, iyakki_core_api::sysinfo::NetUse> {
        crate::sysinfo::net_use(r, pids)
    }
    fn sys_root(&self) -> std::path::PathBuf {
        crate::sysinfo::sys_root().to_path_buf()
    }
}

impl Core for AgentCore {
    fn host(&self) -> &dyn iyakki_core_api::HostFacts {
        &LocalHost
    }
    fn files(&self) -> &dyn iyakki_core_api::Files {
        self
    }
    fn terminal(&self) -> &dyn iyakki_core_api::Terminal {
        &self.terminals
    }
    fn file_scope(&self) -> &dyn iyakki_core_api::FileScope {
        &LocalFileScope
    }
    fn cloud(&self) -> &dyn iyakki_core_api::CloudModes {
        self
    }
    fn flows_view(&self) -> &dyn FlowsView {
        self
    }
    fn home_files(&self) -> &dyn HomeFiles {
        self
    }
    fn vault_view(&self) -> &dyn VaultView {
        self
    }
    fn settings_view(&self) -> &dyn SettingsView {
        self
    }
    fn session_resume(&self) -> &dyn SessionResume {
        self
    }
    fn ui_socket(&self) -> &dyn UiSocket {
        self
    }
    fn app_data(&self) -> &dyn AppData {
        self
    }
    fn desktop(&self) -> &dyn Desktop {
        self
    }
    fn assistant(&self) -> &dyn iyakki_core_api::Assistant {
        match &self.assistant {
            Some(a) => a,
            None => &iyakki_core_api::NoAssistant,
        }
    }
    fn app_actions(&self) -> &dyn iyakki_core_api::AppActions {
        &self.app_actions
    }
    fn app_events(&self) -> &dyn iyakki_core_api::AppEvents {
        &*self.app_events
    }
    fn cameras(&self) -> &dyn iyakki_core_api::Cameras {
        match &self.cameras {
            Some(c) => &**c,
            None => &iyakki_core_api::NoCameras,
        }
    }
    fn media(&self) -> &dyn iyakki_core_api::Media {
        match &self.media {
            Some(m) => m,
            None => &iyakki_core_api::NoMedia,
        }
    }
    fn monitors(&self) -> &dyn Monitors {
        self
    }
    fn browser(&self) -> &dyn iyakki_core_api::BrowserSessions {
        self.embedded.sessions()
    }
    fn mixer(&self) -> &dyn Mixer {
        self
    }
    fn sound(&self) -> &dyn iyakki_core_api::SoundControl {
        self
    }
    fn cues(&self) -> &dyn iyakki_core_api::CuePlayer {
        self
    }
    fn app_status(&self) -> &dyn AppStatuses {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_proxy::{Upstream, UpstreamResponse};

    struct Dead;
    impl Upstream for Dead {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    fn core_in(tag: &str) -> (AgentCore, Arc<Services>, std::path::PathBuf) {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-appstatus-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let svc = Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
        (AgentCore::new(svc.clone()), svc, d)
    }

    #[test]
    fn the_core_answers_file_scope_exactly_as_the_agents_own_checks_do() {
        use iyakki_core_api::FileScope;
        let (core, _svc, d) = core_in("scope");
        for role in [iyakki_sec::policy::Role::Admin, iyakki_sec::policy::Role::User] {
        let pol = iyakki_sec::policy::Policy::resolve(role, &Default::default());
        for p in [d.join("x"), std::path::PathBuf::from("/etc/passwd")] {
            assert_eq!(core.file_scope().check_read(&pol, "alice", &p), crate::scope::check_read(&pol, "alice", &p));
            assert_eq!(core.file_scope().check_write(&pol, "alice", &p), crate::scope::check_write(&pol, "alice", &p));
        }
        }
        assert_eq!(core.file_scope().home_root("alice", &d), crate::scope::home_root("alice", &d));
        let pol = iyakki_sec::policy::Policy::resolve(iyakki_sec::policy::Role::User, &Default::default());
        assert_eq!(core.file_scope().roots(&pol, "alice", &d), crate::scope::file_roots(&pol, "alice", &d));
        assert_eq!(core.file_scope().resolve(&d, "sub/../x"), crate::scope::resolve(&d, "sub/../x"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_core_answers_machine_facts_like_the_agents_own_readers() {
        use iyakki_core_api::HostFacts;
        let (core, _svc, d) = core_in("host");
        assert_eq!(core.host().cmdline(), crate::sysinfo::cmdline(crate::sysinfo::proc_root()));
        assert_eq!(core.host().product_name(), crate::sysinfo::product_name(crate::sysinfo::sys_root()));
        assert_eq!(core.host().proc_root(), std::path::PathBuf::from("/proc"));
        assert_eq!(core.host().mem_total_kb(), crate::sysinfo::mem_total_kb(&crate::sysinfo::SysRoots::real()));
        assert!(core.host().shared_memory_dir().is_dir());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_local_host_reads_the_system_page_figures_like_the_sysinfo_functions_and_refuses_to_signal_process_one() {
        use iyakki_core_api::HostFacts;
        let r = iyakki_core_api::sysinfo::SysRoots::real();
        assert_eq!(super::LocalHost.cpu_count(&r), crate::sysinfo::cpu_count(&r));
        assert_eq!(super::LocalHost.memory_of(&r).total, crate::sysinfo::memory(&r).total);
        assert!(!super::LocalHost.processes(&r).is_empty());
        assert!(super::LocalHost.signal(1, true).is_err());
    }

    #[test]
    fn the_generated_facade_delegates_to_the_agents_own_functions() {
        use iyakki_agent_api::{AgentFiles, AgentOther};
        let api = crate::AgentApiImpl;
        assert_eq!(api.devtools_scrub_url("https://x.test/?token=abc"), crate::devtools::scrub_url("https://x.test/?token=abc"));
        assert_eq!(api.gitconn_host_of("git@example.test:a/b.git"), crate::gitconn::host_of("git@example.test:a/b.git"));
    }

    #[test]
    fn the_flows_facet_remembers_and_forgets_the_last_workflow_and_says_no_scheduler_without_an_engine() {
        let (core, svc, d) = core_in("flowslast");
        let f = core.flows_view();
        assert!(!f.last_flow_recorded("alice") && !f.scheduler_running("alice"), "nothing recorded, nothing running");
        crate::flows::set_last_flow(&svc, "alice", "nightly", "Nightly backup");
        assert!(f.last_flow_recorded("alice"), "the facet reads what the agent's own function wrote");
        f.clear_last_flow("alice");
        assert!(!f.last_flow_recorded("alice") && !crate::flows::last_flow_recorded(&svc, "alice"));
        f.stop_for_logout("alice"); // no engine: nothing to stop, no panic
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_core_decides_cloud_mode_from_the_machines_own_setting() {
        let (core, svc, d) = core_in("cloud");
        let said = core.cloud().detect();
        assert_eq!(said, crate::cloudmode::detect(svc.cloud_setting()), "the facet is the agent's own decision, with the person's setting read by the core");
        assert!(!said.why.is_empty(), "the mode always says why");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_core_says_nothing_when_there_is_nothing_to_say_and_names_overdue_tasks_and_running_jobs_with_the_real_numbers() {
        let (core, svc, d) = core_in("status");
        let said = |c: &AgentCore| -> Vec<(String, AppStatus)> { c.app_status().all("alice") };
        assert!(said(&core).iter().all(|(id, _)| id != "calendar" && id != "runs"), "no overdue task, no job: no status, no made-up figure");
        // an overdue task: CALENDAR needs the person, with the count as the badge
        let now = crate::clock::now_unix();
        svc.inbox.add_task("alice", "Pay the bill", "", Some(now - 3600), crate::inbox::Priority::Normal, "alice", now - 7200).unwrap();
        let (_, cal) = said(&core).into_iter().find(|(id, _)| id == "calendar").expect("calendar says something");
        assert_eq!((cal.kind, cal.count, cal.line.as_str()), (StatusKind::Attention, 1, "1 overdue task"));
        assert_eq!(cal.needs.as_ref().map(|n| n.0.as_str()), Some("Overdue"));
        assert!(core.app_status().all("bob").iter().all(|(id, _)| id != "calendar"), "another person's tasks are not shown");
        // a real job that runs: RUNS says so (a real process, started and stopped through the job manager)
        let work = d.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let spec = crate::jobs::Spec { label: "wait".into(), cmd: crate::jobs::Cmd::Shell("sleep 30".into()), cwd: work, workspace: String::new() };
        let job = svc.jobs.start("alice", &spec, &|_| Ok(())).unwrap();
        let (_, runs) = said(&core).into_iter().find(|(id, _)| id == "runs").expect("runs says something");
        assert_eq!((runs.kind, runs.line.as_str(), runs.needs.clone()), (StatusKind::Running, "1 job running", None));
        svc.jobs.stop("alice", &job.id, std::time::Duration::from_secs(2)).unwrap();
        assert!(said(&core).iter().all(|(id, _)| id != "runs"), "the job ended: the line goes away");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn inbox_cameras_and_the_dev_cluster_say_something_only_when_they_have_something_to_say() {
        assert!(more_statuses(0, &[], false, 0).is_empty());
        let v = more_statuses(2, &["Door".into(), "Yard".into()], true, 3);
        let by = |id: &str| v.iter().find(|(i, _)| i == id).map(|(_, s)| s.clone()).unwrap_or_else(|| panic!("{id} missing"));
        let inbox = by("inbox");
        assert_eq!((inbox.kind, inbox.count, inbox.line.as_str()), (StatusKind::Attention, 2, "2 messages need you"));
        assert_eq!(inbox.needs.as_ref().map(|n| n.0.as_str()), Some("Inbox"));
        assert_eq!(by("cameras").line, "Recording 2 cameras");
        assert_eq!(by("k8s").kind, StatusKind::Running);
        assert_eq!(by("flows").line, "3 flows running");
        assert_eq!(more_statuses(0, &[], false, 1)[0].1.line, "1 flow running");
        assert_eq!(crate::flows::running_for("nobody-has-an-engine"), 0);
        assert_eq!(more_statuses(1, &["Door".into()], false, 0).iter().find(|(i, _)| i == "cameras").unwrap().1.line, "Recording Door");
        assert_eq!(more_statuses(1, &[], false, 0)[0].1.line, "1 message needs you");
    }

    #[test]
    fn the_kinds_map_one_to_one_and_keep_their_priority_order() {
        let all = [SoundKind::Radio, SoundKind::Media, SoundKind::Notification, SoundKind::Speech, SoundKind::Alarm];
        for (i, k) in all.iter().enumerate() {
            assert_eq!(kind(*k).priority() as usize, i);
        }
    }

    #[test]
    fn what_a_tile_says_about_its_icon_comes_back_through_the_core_for_that_person_only_and_clears() {
        let (core, svc, d) = core_in("tilestatus");
        svc.set_tile_status("alice", "sample", Some(3), "3 things\u{7}here");
        let said = |u: &str| core.app_status().all(u).into_iter().find(|(id, _)| id == "sample");
        let (_, st) = said("alice").expect("the tile's words are in the list");
        assert_eq!((st.count, st.line.as_str(), st.kind), (3, "3 thingshere", StatusKind::Running));
        assert!(said("bob").is_none(), "another person does not see it");
        svc.set_tile_status("alice", "sample", Some(7), "x".repeat(100).as_str());
        assert_eq!(said("alice").unwrap().1.line.chars().count(), 40);
        svc.set_tile_status("alice", "sample", None, "");
        assert!(said("alice").is_none(), "an empty status clears it");
        for i in 0..80 {
            svc.set_tile_status("alice", &format!("t{i}"), Some(1), "x");
        }
        assert_eq!(core.app_status().all("alice").len(), 64, "at most 64 tiles are kept per person");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A one-page web server on a free loopback port (a thread of the test).
    fn page_server(body: &'static str) -> u16 {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { break };
                let mut got = Vec::new();
                let mut buf = [0u8; 1024];
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        port
    }

    #[test]
    fn the_device_core_serves_embedded_browser_views_over_the_real_transport_and_obeys_its_switch_and_the_known_bad_list() {
        use iyakki_core_api::browsersession::{BrowserError, Navigate, OpenSpec, Refusal};
        let (core, _svc, d) = core_in("embed-browser");
        let port = page_server("<html><head><title>Embedded</title></head><body><h1>Hello from the loopback</h1></body></html>");
        let b = core.browser();
        let id = b.open("alice", "sample", &OpenSpec::new(320.0, 240.0)).unwrap();
        b.navigate("alice", "sample", id, &Navigate::Url(format!("http://127.0.0.1:{port}/"))).unwrap();
        let t = std::time::Instant::now();
        let st = loop {
            let st = b.state("alice", "sample", id).unwrap();
            if !st.loading || t.elapsed().as_secs() > 15 {
                break st;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!((st.title.as_str(), st.error), ("Embedded", None));
        assert!(b.frame("alice", "sample", id, 0).unwrap().unwrap().list.contains("Hello"));
        // another AgentCore over the same data folder sees the same session (the shell builds more than one handle)
        let again = AgentCore::new(core.services.clone());
        assert!(again.browser().state("alice", "sample", id).is_ok());
        // a site on the person's known-bad list is refused by the core before anything is sent
        let guard = iyakki_web::badsites::Guard::global();
        let mut list = iyakki_web::badsites::BadSites::default();
        list.read_text("embed-bad.example phishing\n");
        guard.set(list, true);
        let e = b.navigate("alice", "sample", id, &Navigate::Url("https://embed-bad.example/".into())).unwrap_err();
        guard.set(iyakki_web::badsites::BadSites::default(), true);
        assert!(matches!(e, BrowserError::Refused(Refusal::KnownBad { .. })), "{e:?}");
        // the person's switch: off ends the views and refuses new ones; on opens again
        b.set_enabled("alice", false).unwrap();
        assert!(!b.enabled("alice"));
        assert_eq!(b.state("alice", "sample", id), Err(BrowserError::NoSuchSession));
        assert_eq!(b.open("alice", "sample", &OpenSpec::new(100.0, 100.0)), Err(BrowserError::Refused(Refusal::Off)));
        assert!(b.open("bob", "sample", &OpenSpec::new(100.0, 100.0)).is_ok(), "the switch is the person's");
        b.set_enabled("alice", true).unwrap();
        assert!(b.enabled("alice") && b.open("alice", "sample", &OpenSpec::new(100.0, 100.0)).is_ok());
        assert_eq!(b.open_views("alice"), 1);
        assert!(b.set_enabled("../x", false).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    struct Everyone;
    impl iyakki_core_api::PolicySource for Everyone {
        fn policy_of(&self, user: &str) -> Option<iyakki_sec::policy::Policy> {
            match user {
                "admin" => Some(iyakki_sec::policy::Role::Admin.template()),
                "guest" => {
                    let mut p = iyakki_sec::policy::Role::Guest.template();
                    p.cli = iyakki_sec::policy::CliLevel::Off;
                    Some(p)
                }
                _ => None,
            }
        }
    }

    #[test]
    fn the_embedded_terminal_belongs_to_one_person_is_limited_in_number_checked_by_policy_and_audited_by_command_word_only() {
        use iyakki_core_api::terminal::{Terminal, MAX_SESSIONS};
        let (core, svc, d) = core_in("terminal");
        let lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let l2 = lines.clone();
        svc.browser.set_audit(Arc::new(move |u: &str, e: &str, d: &str| l2.lock().unwrap().push(format!("{u} {e} {d}"))));
        let core = core.with_policies(Arc::new(Everyone));
        let t = core.terminal();
        // no account, or a terminal switched off for the account: no session
        assert!(t.open("nobody", "app").unwrap_err().contains("no terminal"));
        assert!(t.open("guest", "app").unwrap_err().contains("disabled"));
        let a = t.open("admin", "app").unwrap();
        // another person cannot use or answer this session
        assert!(t.run("guest", &a.id, "ls").is_err());
        assert!(t.run("admin", "t999", "ls").is_err());
        // limits on lines and sessions
        assert!(t.run("admin", &a.id, "").is_err() && t.run("admin", &a.id, "ls\nrm x").is_err());
        let mut ids = vec![a.id.clone()];
        for _ in 1..MAX_SESSIONS {
            ids.push(t.open("admin", "app").unwrap().id);
        }
        assert!(t.open("admin", "app").unwrap_err().contains("at most"));
        t.close("admin", &ids[1]);
        assert!(t.open("admin", "app").is_ok(), "a closed session frees a place");
        // a change waits; the audit names the command word and never the arguments
        let r = t.run("admin", &a.id, "mkdir secret-folder-name").unwrap();
        assert!(r.approval.is_some());
        assert!(t.decide("guest", &a.id, &r.approval.as_ref().unwrap().call, true).is_err(), "another person cannot answer");
        t.decide("admin", &a.id, &r.approval.unwrap().call, false).unwrap();
        t.run("admin", &a.id, "echo private words").unwrap();
        let log = lines.lock().unwrap().join("\n");
        assert!(log.contains("admin terminal.embedded.asked mkdir") && log.contains("terminal.embedded.denied mkdir") && log.contains("terminal.embedded.run echo"), "{log}");
        assert!(!log.contains("secret-folder-name") && !log.contains("private words"), "no arguments in the audit: {log}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_browser_tile_reads_the_home_folder_through_the_real_facet_and_never_leaves_it() {
        use crate::webservice::tilehome::{plan, run};
        let (core, _svc, d) = core_in("tilehome");
        let home = crate::scope::home_root_for_writes("alice", core.services.dir());
        std::fs::create_dir_all(home.join("Docs")).unwrap();
        std::fs::write(home.join("Docs/a.txt"), "hello tile").unwrap();
        std::fs::write(home.join(".secret"), "no").unwrap();
        let outside = d.join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let _ = std::os::unix::fs::symlink(&outside, home.join("link.txt"));
        let ask = |m: &str, a: &str| plan(m, a).and_then(|r| run(&core, "alice", &r));
        let l: serde_json::Value = serde_json::from_str(&ask("list", "{}").unwrap()).unwrap();
        let names: Vec<&str> = l["entries"].as_array().unwrap().iter().filter_map(|e| e["name"].as_str()).collect();
        assert!(names.contains(&"Docs") && !names.contains(&".secret") && !names.contains(&"link.txt"), "{names:?}");
        let t: serde_json::Value = serde_json::from_str(&ask("read", r#"{"path":"Docs/a.txt"}"#).unwrap()).unwrap();
        assert_eq!(t["text"], "hello tile");
        for (m, a) in [("read", r#"{"path":"link.txt"}"#), ("read", r#"{"path":".secret"}"#), ("list", r#"{"dir":"nope"}"#), ("list", r#"{"dir":".."}"#), ("change", "{}")] {
            assert!(ask(m, a).is_err(), "{m} {a}");
        }
        // another person has another home
        let other = plan("list", "{}").and_then(|r| run(&core, "bob", &r)).unwrap();
        assert!(!other.contains("Docs"), "{other}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_browser_tile_runs_a_page_through_the_real_sessions_for_its_own_id_only_and_the_known_bad_list_still_applies() {
        use crate::webservice::tilebrowser::{plan, run};
        let (core, _svc, d) = core_in("tile-browser");
        let port = page_server("<html><head><title>Tile page</title></head><body><h1>Hello from the loopback</h1></body></html>");
        let b = core.shared_browser();
        let call = |tile: &str, user: &str, m: &str, a: &str| plan(m, a).and_then(|r| run(&*b, user, tile, &r));
        let id: serde_json::Value = serde_json::from_str(&call("tile-a", "alice", "open", r#"{"width":320,"height":240}"#).unwrap()).unwrap();
        let n = id["id"].as_u64().unwrap();
        call("tile-a", "alice", "navigate", &format!(r#"{{"id":{n},"kind":"url","url":"http://127.0.0.1:{port}/"}}"#)).unwrap();
        let t = std::time::Instant::now();
        let st = loop {
            let st: serde_json::Value = serde_json::from_str(&call("tile-a", "alice", "state", &format!(r#"{{"id":{n}}}"#)).unwrap()).unwrap();
            if st["loading"] == false || t.elapsed().as_secs() > 15 {
                break st;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(st["title"], "Tile page");
        let fr: serde_json::Value = serde_json::from_str(&call("tile-a", "alice", "frame", &format!(r#"{{"id":{n},"since":0}}"#)).unwrap()).unwrap();
        assert!(fr["list"].as_str().unwrap().contains("Hello"), "{fr}");
        // another tile, or another person, does not reach this session
        assert!(call("tile-b", "alice", "state", &format!(r#"{{"id":{n}}}"#)).is_err());
        assert!(call("tile-a", "bob", "state", &format!(r#"{{"id":{n}}}"#)).is_err());
        // a page asks for nothing here: no prompts, and a yes for a prompt that is not there is refused by the core
        assert_eq!(call("tile-a", "alice", "prompts", &format!(r#"{{"id":{n}}}"#)).unwrap(), "[]");
        assert!(call("tile-a", "alice", "answer", &format!(r#"{{"id":{n},"prompt":99,"allow":false}}"#)).is_err());
        // the known-bad list is the core's, whoever asks
        let guard = iyakki_web::badsites::Guard::global();
        let mut list = iyakki_web::badsites::BadSites::default();
        list.read_text("tile-bad.example phishing\n");
        guard.set(list, true);
        let e = call("tile-a", "alice", "navigate", &format!(r#"{{"id":{n},"kind":"url","url":"https://tile-bad.example/"}}"#));
        guard.set(iyakki_web::badsites::BadSites::default(), true);
        assert!(e.unwrap_err().contains("known-bad"));
        call("tile-a", "alice", "close", &format!(r#"{{"id":{n}}}"#)).unwrap();
        assert!(call("tile-a", "alice", "state", &format!(r#"{{"id":{n}}}"#)).is_err(), "closed");
        let _ = std::fs::remove_dir_all(d);
    }
}
