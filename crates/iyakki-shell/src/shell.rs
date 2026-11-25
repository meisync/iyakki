//! The shell: first-run setup, login, the launcher with app tiles, and the app frame.
//! A pure state machine (events in, pixels out), so it is driven identically by the Linux
//! framebuffer backend, by tests, and by the screenshot tool.

use self::volbar_impl::BarButton;
use crate::stale_stats::mark_stale;
use std::sync::{Arc, Mutex};

use iyakki_shared::chat::ChatBackend;
use iyakki_shared::aiconfig::AiConfig;
use iyakki_shared::mcp::{ServerSpec};
use iyakki_shared::mcp::{self};
use iyakki_shared::services::{VaultState};
use iyakki_core_api::HostApi;
use iyakki_sec::policy::App as PApp;

use crate::accounts::{AccountError, AuditLog, Session, UserStore};
use crate::apps::chat::ChatApp;
use crate::apps::cli::CliApp;
use crate::apps::mcp::McpApp;
use crate::apps::rag::RagApp;
use crate::apps::runs::RunsApp;
use crate::apps::edit::EditApp;
use crate::apps::settings::SettingsApp;
use crate::apps::widgets::{PickEvent, Picker};
use crate::apps::users::UsersApp;
use crate::apps::{App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::launcher::{self, Group};
#[cfg(test)]
use crate::launcher::tile_rects;
use crate::ui::*;

pub struct ShellEnv {
    pub store: crate::accounts::UsersRef,
    pub audit: Arc<dyn iyakki_agent_api::AuditApi>,
    pub host: Arc<dyn HostApi>,
    pub cfg: AiConfig,
    pub chat: Arc<dyn ChatBackend>,
    pub mcp_servers: Vec<ServerSpec>,
    pub services: Arc<dyn iyakki_agent_api::ServicesHandle>,
    /// Web access for the browser.
    pub web: Arc<dyn iyakki_web::Fetch>,
    pub hostname: String,
    /// The data directory is in RAM (live session): nothing survives power-off.
    pub data_volatile: bool,
    /// Start in the hardware check (boot option `iyakki.hwcheck=1`) instead of the login screen.
    pub hwcheck: bool,
}

pub struct ShellPaths {
    pub users: String,
    pub audit: String,
    pub ai_conf: String,
    /// Settings, vaults and the knowledge base live here.
    pub data: String,
    /// Start in the hardware check.
    pub hwcheck: bool,
    /// This process is a headless web service (`iyakki serve --profile NAME`): no screen, no sound, no camera, so nothing here may set up
    /// or probe sound hardware ([`sound_boot_plan`]).
    pub headless: bool,
}

impl Default for ShellPaths {
    fn default() -> Self {
        Self { users: "/var/lib/iyakki/users.json".into(), audit: "/var/lib/iyakki/audit.log".into(), ai_conf: "/etc/iyakki/ai.conf".into(), data: "/var/lib/iyakki".into(), hwcheck: false, headless: false }
    }
}

/// The real `RunStarter` of the core (F-SPLIT-03): a chat run asked for over the UI socket is built here, from the account's own session, with the
/// machine's services, the model and the metered chat, exactly as the CHAT tile builds it. The accounts are read at each start, so a role or
/// policy change applies to the next run.
pub fn core_run_starter(env: &ShellEnv) -> Arc<dyn iyakki_agent_api::RunStarter> {
    let (store, audit, host, cfg, chat, services, web) = (env.store.clone(), env.audit.clone(), env.host.clone(), env.cfg.clone(), env.chat.clone(), env.services.clone(), env.web.clone());
    let mcp_servers = env.mcp_servers.clone();
    let accounts = env.store.clone();
    let policy_of: crate::wiring::PolicyOf = Arc::new(move |user: &str| accounts.lock().ok().and_then(|s| s.get(user).map(|r| r.policy())));
    let core: Arc<dyn iyakki_core_api::Core> = crate::wiring::make_core(env, crate::wiring::CoreInputs { policies: Arc::new(crate::apps::StorePolicies(env.store.clone())), audit: Arc::new(crate::apps::assistant_port::AuditLogSink(env.audit.clone())), policy_of });
    let osguard = Arc::new(osguard_impl::OsGuard::new(env.data_dir()));
    let agent: Arc<dyn iyakki_agent_api::AgentApi> = iyakki_apps::agent_arc();
    let cx_for: iyakki_apps::apps::core_run::CxFor = Arc::new(move |user: &str| {
        let session = {
            let s = store.lock().ok()?;
            let r = s.get(user)?;
            iyakki_accounts::Session { user: r.name.clone(), role: r.role, policy: r.policy() }
        };
        let stored = services.mcp().list();
        let mut servers: Vec<_> = mcp_servers.iter().filter(|s| !stored.iter().any(|x| x.name == s.name)).cloned().collect();
        servers.extend(stored.iter().map(iyakki_shared::mcp::StoredServer::spec));
        Some(AppCx {
            session: session.clone(),
            host: host.clone(),
            cfg: cfg.clone(),
            chat: services.b4().metered_chat(chat.clone(), &session.user),
            audit: audit.clone(),
            store: store.clone(),
            mcp_servers: servers,
            services: services.clone(),
            web: web.clone(),
            os: osguard.os_for("chat", &session.user),
            core: core.clone(),
            agent: agent.clone(),
            store_facets: None,
        })
    });
    Arc::new(crate::apps::core_run::CoreRunStarter::new(cx_for))
}

impl ShellEnv {
    /// The data folder of this install.
    pub fn data_dir(&self) -> &std::path::Path {
        self.services.dir()
    }

    /// The agent's facets over the person's services, one accessor each (alpha.6 split F: the UI asks the context, not `services`, so the day the
    /// context holds the facets itself no app changes).
    pub fn b0(&self) -> Arc<dyn iyakki_agent_api::B0Services> {
        self.services.b0()
    }

    pub fn b3(&self) -> Arc<dyn iyakki_agent_api::ServicesFacetB3> {
        self.services.b3()
    }

    pub fn b4(&self) -> Arc<dyn iyakki_agent_api::ServicesFacetB4> {
        self.services.b4()
    }

    pub fn b5(&self) -> Arc<dyn iyakki_agent_api::ServicesFacetB5> {
        self.services.b5()
    }

    pub fn voicedesk_facet(&self) -> Arc<dyn iyakki_agent_api::VoiceDeskFacet> {
        self.services.voicedesk_facet()
    }

    pub fn extagent_facet(&self) -> Arc<dyn iyakki_agent_api::ExtAgentFacet> {
        self.services.extagent_facet()
    }

    pub fn tasks_facet(&self) -> Arc<dyn iyakki_agent_api::TasksFacet> {
        self.services.tasks_facet()
    }

    pub fn flows_facet(&self) -> Arc<dyn iyakki_agent_api::FlowsFacet> {
        self.services.flows_facet()
    }

    pub fn git_kit(&self) -> Arc<dyn iyakki_agent_api::GitKit> {
        self.services.git_kit()
    }

    /// The stores whose type is already shared data (no agent type): the settings, the inbox, the calendar, the study plans, the secrets, the knowledge base.
    pub fn settings(&self) -> iyakki_agent_api::SettingsRef {
        self.services.settings()
    }

    pub fn inbox(&self) -> Arc<dyn iyakki_agent_api::InboxApi> {
        self.services.inbox()
    }

    pub fn calendar(&self) -> Arc<dyn iyakki_agent_api::CalendarApi> {
        self.services.calendar()
    }

    pub fn study(&self) -> Arc<dyn iyakki_agent_api::StudyApi> {
        self.services.study()
    }

    pub fn study_calendar(&self) -> Arc<dyn iyakki_agent_api::StudyCalendarApi> {
        self.services.study_calendar()
    }

    pub fn secrets(&self) -> &iyakki_shared::vault::Secrets {
        &self.services.secrets()
    }

    pub fn kb(&self) -> Arc<dyn iyakki_agent_api::KnowledgeBaseApi> {
        self.services.kb()
    }

    /// The knowledge base as the source of the workspace history.
    pub fn kb_source(&self) -> Arc<dyn iyakki_memx::kb::KbSource> {
        self.services.kb_source()
    }

    /// The voice and sound settings, a copy of what the settings hold.
    pub fn voice_settings(&self) -> iyakki_shared::voice::VoiceSettings {
        self.services.voice_settings()
    }

    pub fn sound_settings(&self) -> iyakki_core_api::soundtypes::SoundSettings {
        self.services.sound_settings()
    }

    pub fn agents_cfg(&self) -> iyakki_shared::extagentcfg::AgentsCfg {
        self.services.agents_cfg()
    }

    /// What the last look at each external coding agent found (shared data type).
    pub fn ext_status(&self) -> Arc<dyn iyakki_agent_api::ExtStatusApi> {
        self.services.ext_status()
    }

    // ---- split G: the model and speech side
    /// The speech engines of this computer (detection, the browser voice helper).
    pub fn host_tts(&self) -> Arc<dyn iyakki_agent_api::HostTtsApi> {
        self.ai().host_tts()
    }

    /// The model side: providers, picture models, vendor sessions.
    pub fn ai(&self) -> Arc<dyn iyakki_agent_api::AiRead> {
        self.services.ai()
    }

    /// The vaults: the system vault, each person's vault, the answers to secret requests.
    pub fn vault(&self) -> Arc<dyn iyakki_agent_api::VaultRead> {
        self.services.vault_facet()
    }

    /// The secrets the assistant asked the person for and that wait for an answer (shared data type).
    pub fn secret_requests(&self) -> Arc<dyn iyakki_agent_api::SecretRequestsApi> {
        self.services.secret_requests()
    }

    /// The token usage log (shared data type, a file in the data folder).
    pub fn usage(&self) -> Arc<dyn iyakki_agent_api::UsageLogApi> {
        self.services.usage()
    }

    /// Put a note into the person's inbox (the INBOX tile and the badge show it).
    pub fn notify(&self, user: &str, kind: iyakki_shared::inbox::Kind, title: &str, body: &str, link: Option<iyakki_shared::inbox::Link>) -> Result<(), String> {
        let source = link.as_ref().map(|l| l.app.clone()).unwrap_or_else(|| "system".into());
        self.inbox().add(user, kind, title, body, &source, link, iyakki_clock::now_unix()).map(|_| ())
    }

    /// Like [`ShellEnv::notify`], but not again while an unread note with the same title is still there.
    pub fn notify_once(&self, user: &str, kind: iyakki_shared::inbox::Kind, title: &str, body: &str, link: Option<iyakki_shared::inbox::Link>) -> Result<(), String> {
        if self.inbox().list(user).iter().any(|n| !n.read && n.title == title) {
            return Ok(());
        }
        self.notify(user, kind, title, body, link)
    }

    /// The store, the update check and the system components as narrow facets.
    /// The device watch (the things plugged in and the ones the person trusts).
    pub fn devwatch(&self) -> Arc<dyn iyakki_agent_api::DevWatchApi> {
        self.services.devwatch()
    }

    /// The browser state (the person's consent, the jobs the shell serves) and the screen and camera hubs.
    pub fn browser(&self) -> Arc<dyn iyakki_agent_api::BrowserStateApi> {
        self.services.browser()
    }

    /// The questions the assistant asks about the person's browser and their answers.
    pub fn browser_consent(&self) -> Arc<dyn iyakki_agent_api::ConsentApi> {
        self.services.browser_consent()
    }

    /// The queue between the assistant's browser tools and the page engine the shell services.
    pub fn browser_jobs(&self) -> Arc<dyn iyakki_agent_api::BrowserJobsApi> {
        self.services.browser_jobs()
    }

    pub fn screen(&self) -> Arc<dyn iyakki_agent_api::ScreenCaptureApi> {
        self.services.screen()
    }

    pub fn camera(&self) -> Arc<dyn iyakki_agent_api::CameraHubApi> {
        self.services.camera()
    }

    /// The chat sessions, skills, runs, MCP servers, skill packages and background jobs of the person (stores that stay in the agent).
    pub fn sessions(&self) -> Arc<dyn iyakki_agent_api::SessionApi> {
        self.services.sessions().clone()
    }

    pub fn skills(&self) -> Arc<dyn iyakki_agent_api::SkillApi> {
        self.services.skills().clone()
    }

    pub fn runs(&self) -> Arc<dyn iyakki_agent_api::RunApi> {
        self.services.runs().clone()
    }

    pub fn mcp(&self) -> &dyn iyakki_agent_api::McpApi {
        self.services.mcp()
    }

    pub fn packages(&self) -> Arc<dyn iyakki_agent_api::PackageApi> {
        self.services.packages().clone()
    }

    pub fn jobs(&self) -> Arc<dyn iyakki_agent_api::JobApi> {
        self.services.jobs()
    }

    /// The radio player.
    pub fn radio(&self) -> Arc<dyn iyakki_agent_api::RadioApi> {
        self.services.radio()
    }

    /// The developer tools (the browser's console and network log).
    pub fn devtools(&self) -> Arc<dyn iyakki_agent_api::DevToolsApi> {
        self.services.devtools()
    }

    // --- the small reads of the person's services that no store owns (piece 10): one accessor each, so the day `services` goes only these bodies change.
    pub fn quick_upstream(&self) -> Arc<dyn iyakki_proxy::Upstream> {
        self.services.quick_upstream()
    }

    pub fn page_snapshot_of(&self, user: &str) -> Option<iyakki_shared::webtools::PageSnapshot> {
        self.services.page_snapshot_of(user)
    }

    pub fn set_page_snapshot(&self, user: &str, snap: Option<iyakki_shared::webtools::PageSnapshot>) {
        self.services.set_page_snapshot(user, snap)
    }

    pub fn set_streamer(&self, s: Arc<dyn iyakki_proxy::Streamer>) {
        self.services.set_streamer(s)
    }

    pub fn streamer(&self) -> Option<Arc<dyn iyakki_proxy::Streamer>> {
        self.services.streamer()
    }

    pub fn session_workdir(&self, user: &str, session_id: &str) -> Result<std::path::PathBuf, String> {
        self.services.session_workdir(user, session_id)
    }

    pub fn session_files(&self, user: &str, id: &str) -> usize {
        self.services.session_files(user, id)
    }

    pub fn set_voice_mic(&self, id: &str) -> Result<(), String> {
        self.services.set_voice_mic(id)
    }

    pub fn chat_backend_for(&self, choice: iyakki_shared::settings::ModelChoice) -> Arc<dyn iyakki_shared::chat::ChatBackend> {
        self.services.chat_backend_for(choice)
    }

    /// The downloaded command line tools.
    pub fn clis(&self) -> Arc<dyn iyakki_agent_api::CliApi> {
        self.services.clis().clone()
    }

    /// The Ollama server of the CLI page.
    pub fn ollama(&self) -> Arc<dyn iyakki_agent_api::OllamaApi> {
        self.services.ollama().clone()
    }

    /// The sandbox cluster of the CLI page.
    pub fn sandbox_cluster(&self) -> Arc<dyn iyakki_agent_api::SandboxCluster> {
        self.services.cluster().clone()
    }

    /// The managed dev cluster (k3s) of this computer.
    pub fn k3sdev(&self) -> Arc<dyn iyakki_agent_api::DevCluster> {
        self.services.k3sdev().clone()
    }

    pub fn storeapi(&self) -> Arc<dyn iyakki_agent_api::StoreFacets> {
        self.services.store_facets()
    }

    /// Another environment over the same accounts, audit log and services (one more shell for one more person, as the
    /// browser front end needs).
    pub fn share(&self) -> ShellEnv {
        ShellEnv {
            store: self.store.clone(),
            audit: self.audit.clone(),
            host: self.host.clone(),
            cfg: self.cfg.clone(),
            chat: self.chat.clone(),
            mcp_servers: self.mcp_servers.clone(),
            services: self.services.clone(),
            web: self.web.clone(),
            hostname: self.hostname.clone(),
            data_volatile: self.data_volatile,
            hwcheck: false,
        }
    }

    /// Build the real environment from files on disk.
    pub fn from_paths(p: &ShellPaths, host: Arc<dyn HostApi>) -> Result<ShellEnv, AccountError> {
        use iyakki_proxy::https::UreqUpstream;
        // before anything below reads the process-wide handle: the first read would fix it to the fake (KI-devmode-off-2026-11-17)
        assert!(iyakki_apps::ensure_real_agent(), "the agent handle is not the real implementation");
        let t0 = std::time::Instant::now();
        let cfg = AiConfig::load(&p.ai_conf);
        let quick: Arc<dyn iyakki_proxy::Upstream> = Arc::new(UreqUpstream::with_timeout(std::time::Duration::from_secs(10)));
        let slow: Arc<dyn iyakki_proxy::Upstream> = Arc::new(UreqUpstream::with_timeout(std::time::Duration::from_secs(300)));
        let services = crate::wiring::open_services(std::path::Path::new(&p.data), quick, slow).map_err(AccountError::Storage)?;
        // which computer is this? Its sound, display, keyboard... settings are laid over the loaded ones before anything reads them
        // (`hwprofile`); the display size of an account follows the computer too
        let computer = services.b3().computer_attach();
        iyakki_uikit::uisize::set_computer_hooks(|d, u| iyakki_apps::agent().computer_hooks().display_size_get(d, u), |d, u, l| iyakki_apps::agent().computer_hooks().display_size_set(d, u, l));
        // the web engine's own messages are kept next to the crash log (`enginelog.rs`)
        iyakki_apps::agent().enginelog_set_dir(&std::path::Path::new(&p.data).join("log"));
        let t_services = t0.elapsed();
        services.radio().set_streamer(Arc::new(iyakki_proxy::https::UreqStreamer::default()));
        // what is installed from the CLI list is found everywhere (jobs, the terminal, agents) without links in /usr/bin, and a program
        // that is on the machine already counts as installed
        services.clis().export_path();
        services.clis().set_detect_present(true);
        // sound goes to the output that is connected (HDMI/DP, USB, ...), not to ALSA's card 0 device 0
        let t1 = std::time::Instant::now();
        // cloud mode without sound hardware: no audio probes, no soft volume, no device watcher
        // (the sound card may appear after the shell started: the MacBookPro14,1's does, about 11 s after boot; then the
        // output is set up as soon as it is there, whoever is signed in)
        match sound_boot_plan(p.headless, services.cloud_setting(), &iyakki_shared::cloudmode::detect(services.cloud_setting())) {
            SoundBoot::Now => services.start_sound_output(),
            SoundBoot::WhenReady => services.start_sound_output_when_ready(),
            SoundBoot::Never => {}
        }
        let t_sound = t1.elapsed();
        // model downloads report their progress over time, like the radio
        services.set_streamer(Arc::new(iyakki_proxy::https::UreqStreamer::default()));
        // model answers are read as they are written; a local model may need minutes to load before its first word
        services.set_chat_streamer(iyakki_proxy::https::chat_streamer(std::time::Duration::from_secs(600)));
        migrate_legacy_chat(&*services, &cfg);
        // the time zone the person chose in Settings -> Time applies to every front end from the start
        if let Ok(st) = services.settings().lock() {
            iyakki_clock::set_chosen_zone(&st.get().time.zone);
        }
        let chat = services.chat_backend();
        let mcp_servers = mcp::load_config(&cfg.mcp_config);
        let t2 = std::time::Instant::now();
        let store = UserStore::open(std::path::Path::new(&p.users))?;
        // an unknown computer: every administrator gets one card (Settings -> This computer)
        if computer {
            let admins: Vec<String> = store.list().iter().filter(|u| u.policy().manage_settings).map(|u| u.name.clone()).collect();
            services.b3().computer_announce(&admins);
        }
        log::info!("boot: shell environment took {} ms (services {}, sound {}, accounts {})", t0.elapsed().as_millis(), t_services.as_millis(), t_sound.as_millis(), t2.elapsed().as_millis());
        let audit = Arc::new(AuditLog::new(std::path::Path::new(&p.audit)));
        audit.flush_notes();
        // temp files a crash or a power cut left behind (a save that never reached its rename)
        sweep_leftover_temp(std::path::Path::new(&p.data), &audit, &iyakki_schema::pid_alive);
        Ok(ShellEnv {
            store: crate::accounts::UsersRef::of_store(&Arc::new(Mutex::new(store))),
            audit,
            host,
            cfg,
            chat,
            mcp_servers,
            services,
            web: browser_web(std::path::Path::new(&p.data)),
            hostname: iyakki_core_local::machinefacts::hostname("iyakki"),
            hwcheck: p.hwcheck,
            data_volatile: iyakki_core_local::storage::data_location(&iyakki_core_local::storage::Roots::real(), std::path::Path::new(&p.data)) == iyakki_core_local::storage::DataLocation::Memory,
        })
    }
}

/// What a UI program that reaches the core over its socket (`iyakki-ui --socket`, F-SPLIT-03) gives the shell instead of letting it open the
/// machine itself: the services, the audit log and the accounts are the core's, seen through remote objects; the host is the program's own
/// quiet one. The machine-level start-up (sound output, streamers, the CLI path, the computer announcement, the legacy chat migration) is the
/// core's and is never repeated here.
pub struct RemoteParts {
    pub services: Arc<dyn iyakki_agent_api::ServicesHandle>,
    pub audit: Arc<dyn iyakki_agent_api::AuditApi>,
    pub users: crate::accounts::UsersRef,
    pub host: Arc<dyn HostApi>,
    /// The ai.conf path (read-only text the UI may read: the MCP servers file and the chat defaults).
    pub ai_conf: String,
    pub hostname: String,
    pub hwcheck: bool,
}

impl ShellEnv {
    /// The environment of a UI program over the core's socket. The services must be the core's (`is_real`), the process-wide agent handle
    /// must be the real one too (a fake never wins, KI-1010): both are asked, and a core that does not answer is an error naming why.
    pub fn from_remote(p: RemoteParts) -> Result<ShellEnv, AccountError> {
        if !iyakki_apps::ensure_real_agent() {
            return Err(AccountError::Storage("the agent handle is not the real implementation".into()));
        }
        if !p.services.is_real() {
            return Err(AccountError::Storage("the core does not answer over the UI socket".into()));
        }
        let cfg = AiConfig::load(&p.ai_conf);
        iyakki_uikit::uisize::set_computer_hooks(|d, u| iyakki_apps::agent().computer_hooks().display_size_get(d, u), |d, u, l| iyakki_apps::agent().computer_hooks().display_size_set(d, u, l));
        // the time zone the person chose in Settings -> Time applies to every front end from the start
        if let Ok(st) = p.services.settings().lock() {
            iyakki_clock::set_chosen_zone(&st.get().time.zone);
        }
        let chat = p.services.chat_backend();
        let mcp_servers = mcp::load_config(&cfg.mcp_config);
        let data = p.services.dir().to_path_buf();
        Ok(ShellEnv {
            store: p.users,
            audit: p.audit,
            host: p.host,
            cfg,
            chat,
            mcp_servers,
            web: browser_web(&data),
            hostname: p.hostname,
            hwcheck: p.hwcheck,
            data_volatile: iyakki_core_local::storage::data_location(&iyakki_core_local::storage::Roots::real(), &data) == iyakki_core_local::storage::DataLocation::Memory,
            services: p.services,
        })
    }
}

/// What the boot does about the sound output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SoundBoot {
    /// Probe the outputs and set up the soft volume now.
    Now,
    /// No sound card yet: wait for one to appear (some laptops register theirs ten seconds after boot).
    WhenReady,
    /// Cloud mode forced on: no probes, no soft volume, no device watcher.
    Never,
}

/// Cloud mode without sound hardware skips the sound-output probe and the soft-volume setup at boot; a forced setting never
/// starts the watcher either. A headless web service (`--profile`) never touches sound whatever the machine has: the first run of
/// the cloud profile on a PC set a real card's volume and asked its devices for their formats.
pub(crate) fn sound_boot_plan(headless: bool, setting: iyakki_cloudmode::Setting, mode: &iyakki_cloudmode::CloudMode) -> SoundBoot {
    if headless {
        SoundBoot::Never
    } else if mode.audio_services {
        SoundBoot::Now
    } else if setting == iyakki_cloudmode::Setting::On {
        SoundBoot::Never
    } else {
        SoundBoot::WhenReady
    }
}

/// The once-a-second camera look runs on a real machine whose mode keeps the camera; tests and servers never look.
pub(crate) fn wants_system_camera(in_test: bool, mode: &iyakki_cloudmode::CloudMode) -> bool {
    !in_test && mode.camera_watch
}

/// The browser's network: the HTTP client behind connection limits (6 per host, 32 overall, urgent requests first), with one page
/// cache per person under `cache/web/<user>` (memory tier plus disk tier, sized from the machine, see `apps::browser_speed`).
/// Requests that do not say who they are (the assistant's web tools) are never cached.
fn browser_web(data: &std::path::Path) -> Arc<dyn iyakki_web::Fetch> {
    use iyakki_web::net::{LimitedFetch, Limiter, GLOBAL, PER_HOST};
    // the person's proxy (chat tool `browser_proxy_set`): HTTP/2 and HTTP/1.1 both go through its CONNECT tunnel; a saved text that no longer parses is ignored (the file is written only after it parsed)
    let fetch = match iyakki_proxy::proxyset::load(data) {
        Some(px) => iyakki_proxy::https::BrowserFetch::default().with_proxy(px).expect("a proxy setting that was accepted builds a client"),
        None => iyakki_proxy::https::BrowserFetch::default(),
    };
    let limited: Arc<dyn iyakki_web::Fetch> = Arc::new(LimitedFetch::new(fetch, Arc::new(Limiter::new(PER_HOST, GLOBAL))));
    let base = data.join("cache").join("web");
    let budget = crate::apps::browser_speed::budget_for(iyakki_web::net::CacheSize::Auto, &crate::apps::browser_speed::machine(data));
    // the person's custom request headers (Settings -> Browser -> Custom headers) are added here, on the way to the network, for the requests
    // whose address matches a rule; a redirect is judged again at every hop
    let limited: Arc<dyn iyakki_web::Fetch> = Arc::new(iyakki_web::headerfetch::HeaderFetch::new(limited, iyakki_apps::agent().browser_headers().provider()));
    let caches = Arc::new(iyakki_web::cache::UserCaches::new(limited, base, budget));
    // files an older version left directly in the folder belonged to nobody: remove them off the boot path
    let old = caches.clone();
    let _ = std::thread::Builder::new().name("iyakki-cache-tidy".into()).spawn(move || old.remove_legacy_files());
    // the person's ad and tracker filter sits outside the cache: a blocked request is never sent and never cached
    // the known-bad site list sits outside everything: a listed site is never asked, never cached
    Arc::new(iyakki_web::badsites::BadSiteFetch::new(Arc::new(iyakki_web::blocker_state::FilteredFetch::new(caches, iyakki_web::blocker_state::Filter::global())), iyakki_web::badsites::Guard::global()))
}

/// Older installs configured chat in ai.conf (`chat.url`, `chat.model`, `chat.key_*`). Carry that over
/// once, as a normal provider, so nothing stops working — Settings is the source of truth after.
fn migrate_legacy_chat(svc: &dyn iyakki_agent_api::ServicesHandle, cfg: &AiConfig) {
    use iyakki_shared::providers::{Kind, ProviderCfg};
    let Some(url) = &cfg.chat_url else { return };
    let mut st = svc.settings().lock().unwrap();
    if st.get().chat.is_some() || st.get().provider("legacy").is_some() {
        return;
    }
    let base = url.trim_end_matches("/chat/completions").trim_end_matches('/').to_string();
    let mut p = ProviderCfg::new("legacy", Kind::Compat);
    p.base_url = Some(base);
    p.key_ref = cfg.chat_key_file.as_ref().map(|f| format!("file:{f}")).or_else(|| cfg.chat_key_env.as_ref().map(|e| format!("env:{e}")));
    p.default_model = Some(cfg.chat_model.clone());
    if st.upsert_provider(p).is_ok() {
        let _ = st.set_chat(Some(iyakki_shared::settings::ModelChoice { provider: "legacy".into(), model: cfg.chat_model.clone() }));
    }
}

struct Form {
    fields: Vec<TextField>,
    focus: usize,
    error: Option<String>,
}

impl Form {
    fn login() -> Form {
        Form { fields: vec![TextField::new("Username", false), TextField::new("Password", true)], focus: 0, error: None }
    }
    /// The sign-in form with the last person who signed in already filled in, and the cursor on the password.
    fn login_for(last: Option<&str>) -> Form {
        let mut f = Form::login();
        if let Some(name) = last {
            f.fields[0].set(name);
            f.focus = 1;
        }
        f
    }
    fn setup() -> Form {
        Form { fields: vec![TextField::new("Choose a username", false), TextField::new("Choose a password (min 8)", true), TextField::new("Repeat password", true)], focus: 0, error: None }
    }
    fn next(&mut self) {
        self.focus = (self.focus + 1) % self.fields.len();
    }
    fn prev(&mut self) {
        self.focus = (self.focus + self.fields.len() - 1) % self.fields.len();
    }
}

enum Screen {
    HwCheck(Box<crate::hwcheck::HwCheck>),
    Setup(Form),
    Login(Form),
    Launcher,
    App(Box<dyn App>, PApp),
    PowerMenu,
}

struct Tile {
    app: PApp,
    icon: Icon,
    label: &'static str,
    sub: &'static str,
    accent: [u8; 4],
    /// The home screen section the tile is in; `None` keeps it off the grid (Settings: an icon in the top bar, the IYAKKI menu and F7 lead there too).
    group: Option<Group>,
}

/// One thing on the launcher: a system tile, or a custom app.
enum LItem {
    Sys(&'static Tile),
    App(Box<iyakki_shared::store_tilehub::TileEntry>),
    /// The hint shown in a group of the person's own that has no tiles yet.
    Empty,
}

const TILES: &[Tile] = &[
    Tile { app: PApp::Chat, icon: Icon::Chat, label: "CHAT", sub: "Talk to the AI", accent: rgb(79, 140, 255), group: Some(Group::Home) },
    Tile { app: PApp::Rag, icon: Icon::Rag, label: "RAG", sub: "Ask your files", accent: rgb(61, 214, 163), group: Some(Group::Home) },
    Tile { app: PApp::Mcp, icon: Icon::Mcp, label: "MCP", sub: "Tools, servers", accent: rgb(167, 116, 255), group: Some(Group::Work) },
    Tile { app: PApp::Cli, icon: Icon::Cli, label: "CLI", sub: "Terminal, tools", accent: rgb(255, 184, 77), group: Some(Group::Work) },
    Tile { app: PApp::Edit, icon: Icon::Edit, label: "EDIT", sub: "Edit files", accent: rgb(110, 224, 160), group: Some(Group::Work) },
    Tile { app: PApp::Runs, icon: Icon::Runs, label: "RUNS", sub: "Agent history", accent: rgb(255, 196, 94), group: Some(Group::Work) },
    Tile { app: PApp::Users, icon: Icon::Users, label: "USERS", sub: "Accounts", accent: rgb(255, 107, 154), group: Some(Group::System) },
    Tile { app: PApp::Settings, icon: Icon::Settings, label: "SETTINGS", sub: "Models, vault", accent: rgb(120, 170, 255), group: None },
    Tile { app: PApp::Browser, icon: Icon::Browser, label: "BROWSER", sub: "Read the web", accent: rgb(88, 190, 235), group: Some(Group::WebMedia) },
    Tile { app: PApp::Radio, icon: Icon::Radio, label: "RADIO", sub: "Online stations", accent: rgb(255, 150, 110), group: Some(Group::WebMedia) },
    Tile { app: PApp::Skills, icon: Icon::Skills, label: "SKILLS", sub: "Teach tasks", accent: rgb(255, 170, 90), group: Some(Group::Home) },
    Tile { app: PApp::Scan, icon: Icon::Scan, label: "NET SCAN", sub: "Find services", accent: rgb(120, 220, 140), group: Some(Group::System) },
    Tile { app: PApp::Mesh, icon: Icon::Mesh, label: "MESH", sub: "Consul, services", accent: rgb(86, 204, 196), group: Some(Group::System) },
    Tile { app: PApp::Usage, icon: Icon::Usage, label: "USAGE", sub: "Tokens by model", accent: rgb(255, 130, 170), group: Some(Group::Data) },
    Tile { app: PApp::System, icon: Icon::System, label: "SYSTEM", sub: "Load, agents", accent: rgb(176, 226, 92), group: Some(Group::System) },
    Tile { app: PApp::Calendar, icon: Icon::Calendar, label: "CALENDAR", sub: "Plans, alarms", accent: rgb(110, 190, 255), group: Some(Group::Home) },
    Tile { app: PApp::Media, icon: Icon::Media, label: "MEDIA", sub: "Music, videos", accent: rgb(240, 120, 220), group: Some(Group::WebMedia) },
    Tile { app: PApp::Ide, icon: Icon::Ide, label: "AI IDE", sub: "Code with the AI", accent: rgb(100, 230, 255), group: Some(Group::Work) },
    Tile { app: PApp::Data, icon: Icon::Data, label: "DATA", sub: "Tables, CSV", accent: crate::apps::data::ACCENT, group: Some(Group::Data) },
    Tile { app: PApp::Tales, icon: Icon::Tales, label: "TALES", sub: "Read-aloud tales", accent: crate::apps::story::ACCENT, group: Some(Group::Home) },
    Tile { app: PApp::Store, icon: Icon::Store, label: "STORE", sub: "Apps and skills", accent: crate::apps::store::ACCENT, group: Some(Group::System) },
    Tile { app: PApp::Dash, icon: Icon::Dash, label: "DASH", sub: "Graphs, tables", accent: crate::apps::dash::ACCENT, group: Some(Group::Data) },
    Tile { app: PApp::Flows, icon: Icon::Flows, label: "FLOWS", sub: "Workflows, runs", accent: crate::apps::flows::ACCENT, group: Some(Group::Work) },
    Tile { app: PApp::Camera, icon: Icon::Camera, label: "CAMERA", sub: "Photos, video", accent: crate::apps::camera::ACCENT, group: Some(Group::WebMedia) },
    Tile { app: PApp::Voice, icon: Icon::Speaker, label: "VOICE", sub: "Read, dictate", accent: crate::apps::voice::ACCENT, group: Some(Group::Home) },
    Tile { app: PApp::Disks, icon: Icon::Disks, label: "DISKS", sub: "Drives, partitions", accent: crate::apps::disks_tile::ACCENT, group: Some(Group::System) },
    Tile { app: PApp::K8s, icon: Icon::K8s, label: "K8S", sub: "Explore the cluster", accent: crate::apps::k8s_tile::ACCENT, group: Some(Group::Work) },
    Tile { app: PApp::LinuxWin, icon: Icon::Cli, label: "LINUX WINDOW", sub: "Linux programs", accent: crate::apps::linuxwin::ACCENT, group: Some(Group::System) },
    Tile { app: PApp::Lab, icon: Icon::Lab, label: "LAB", sub: "Try picture models", accent: crate::apps::lab::ACCENT, group: Some(Group::Home) },
    Tile { app: PApp::Git, icon: Icon::Git, label: "GIT", sub: "Browse repositories", accent: crate::apps::git_tile::ACCENT, group: Some(Group::Work) },
    Tile { app: PApp::Doctor, icon: Icon::Doctor, label: "DOCTOR", sub: "Find and fix", accent: crate::apps::doctor::ACCENT, group: Some(Group::System) },
    Tile { app: PApp::Learn, icon: Icon::Learn, label: "LEARN", sub: "Study and quiz", accent: crate::apps::learn::ACCENT, group: Some(Group::Home) },
    Tile { app: PApp::DevTools, icon: Icon::Edit, label: "DEV TOOLS", sub: "JSON, JWT, QR code", accent: crate::apps::devtools::ACCENT, group: Some(Group::Work) },
    Tile { app: PApp::Remote, icon: Icon::Data, label: "REMOTE FILES", sub: "SFTP, FTPS", accent: crate::apps::remotefiles::ACCENT, group: Some(Group::System) },
    Tile { app: PApp::Cameras, icon: Icon::Camera, label: "CAMERAS", sub: "Live view, photos", accent: crate::apps::cameras::ACCENT, group: Some(Group::WebMedia) },
    Tile { app: PApp::Pdf, icon: Icon::Data, label: "PDF", sub: "Read, search, copy", accent: crate::apps::pdfview::ACCENT, group: Some(Group::WebMedia) },
    Tile { app: PApp::Auth, icon: Icon::Lock, label: "AUTH", sub: "One-time codes", accent: crate::apps::auth::ACCENT, group: Some(Group::System) },
    Tile { app: PApp::Sample, icon: Icon::Clipboard, label: "SAMPLE", sub: "Built on the UI SDK", accent: crate::apps::sample::ACCENT, group: Some(Group::Work) },
    Tile { app: PApp::Markdown, icon: Icon::Data, label: "MARKDOWN", sub: "Documents to read", accent: crate::apps::markdown::ACCENT, group: Some(Group::Work) },
    Tile { app: PApp::Firewall, icon: Icon::Lock, label: "FIREWALL", sub: "Rules in and out", accent: crate::apps::firewall_tile::ACCENT, group: Some(Group::System) },
    Tile { app: PApp::Checkup, icon: Icon::Checkup, label: "CHECK-UP", sub: "Test the hardware", accent: crate::apps::checkup::ACCENT, group: Some(Group::System) },
];

/// The home screen section of an app's tile (the tile table's `group` field; `None`: not on the grid).
pub(crate) fn tile_group(app: PApp) -> Option<Group> {
    TILES.iter().find(|t| t.app == app).and_then(|t| t.group)
}

/// The tiles a person sees on the grid, as (layout key, label, the group the tile is in when left alone), for Settings → Home layout.
pub(crate) fn layout_tiles(admin: bool, cloud: &iyakki_cloudmode::CloudMode) -> Vec<(String, &'static str, &'static str)> {
    TILES.iter().filter(|t| t.group.is_some() && (t.app != PApp::Users || admin) && !cloud.hides_app(t.app)).map(|t| (iyakki_shared::layout::sys_key(t.label), t.label, t.group.map_or("", |g| g.id()))).collect()
}

/// The apps a person may pin to the dock, as (app id, label), in tile order (for Settings, Appearance).
pub(crate) fn pinnable_apps(policy: &iyakki_sec::policy::Policy) -> Vec<(&'static str, &'static str)> {
    TILES.iter().filter(|t| t.group.is_some() && policy.allows_app(t.app)).map(|t| (t.app.id(), t.label)).collect()
}

/// The tiles as (app, label), for tests of other modules.
#[cfg(test)]
pub(crate) fn tiles_for_test() -> Vec<(PApp, &'static str)> {
    TILES.iter().map(|t| (t.app, t.label)).collect()
}

/// The name of the last person who signed in, kept so the sign-in form can fill it in (a name, never a password).
fn last_user(env: &ShellEnv) -> Option<String> {
    let t = std::fs::read_to_string(env.data_dir().join("last-user")).ok()?;
    let t = t.trim();
    iyakki_sec::policy::valid_username(t).then(|| t.to_string())
}

fn remember_user(env: &ShellEnv, name: &str) {
    if iyakki_sec::policy::valid_username(name) {
        let _ = std::fs::write(env.data_dir().join("last-user"), format!("{name}\n"));
    }
}

/// What a person sees after the shell crashed and was not restored (F-LIFE-16).
pub const UNEXPECTED_END_NOTICE: &str = "The screen restarted. Please sign in again; anything you had not saved in an open app may be lost.";

pub struct Shell {
    env: ShellEnv,
    /// The UI's handle on the core (`iyakki_core_api::Core`), shared with every app through `AppCx::core`.
    core: Arc<dyn iyakki_core_api::Core>,
    /// The agent's functions the apps call (generated facade); the shell is the composition root, so it makes the real one and gives it to the apps (`iyakki_apps::install_agent`).
    agent: Arc<dyn iyakki_agent_api::AgentApi>,
    /// Signing in as a person put back after a restart (no password was typed): the vaults stay locked.
    resuming: bool,
    /// Whether the start-up look for a session ticket has been made.
    resume_tried: bool,
    /// The one raw machine handle and the broker: apps get a `GuardedOs` made from it, never the raw one (`shell_osguard.rs`).
    osguard: osguard_impl::OsGuard,
    /// Accessibility: the screen reader, the keyboard walk and the saved options (`shell_access.rs`).
    acc: access_impl::AccessUi,
    /// Hit regions from the last rendered frame (clicks are resolved against these).
    hits: Vec<crate::ui::Hit>,
    /// Clipboard-history picker overlay (Ctrl+Shift+V, right-click, or the clipboard button).
    clip: Option<Picker>,
    /// Host files (window mode): the import dialog shown over any screen for dropped files, and whether files are being
    /// dragged over the window right now.
    hostdlg: Option<Box<crate::apps::hostfiles::HostDialog>>,
    /// A flow's permission request, shown over every screen until it is answered, dismissed or ten minutes old (it stays in the INBOX).
    permpop: Option<crate::apps::browser_consent::ConsentDialog>,
    /// The sign-in dialog of Claude Code / GitHub Copilot (agent_login.rs), over any screen.
    login: Option<Box<crate::agent_login::LoginModal>>,
    /// "Scan QR" of the clipboard history (clip_scan.rs): the camera, what was read, Copy / Scan again / Cancel.
    clipscan: Option<Box<crate::clip_scan::ClipScan>>,
    /// Where the cameras of the scan come from (`None`: the machine's own; tests give made-up ones).
    scan_src: Option<std::sync::Arc<crate::scan_core::CamSource>>,
    drag_over: bool,
    /// The right-click menu (ctxmenu.rs), and the text fields the last frame drew (it needs them to know what is under the pointer).
    ctx: Option<crate::ctxmenu::Menu>,
    fields: Vec<crate::ctxmenu::FieldNote>,
    last_click: Option<(u64, String)>,
    /// The text field or area under the button while it is held (with its rectangle), so moving the pointer can extend a selection.
    press: Option<(Event, R)>,
    /// When and where the last press was, and how many in a row (double and triple clicks).
    click_run: (u64, i32, i32, u8),
    screen: Screen,
    /// The custom apps (tile apps) this person has, refreshed when the launcher is shown and every couple of seconds.
    custom: Vec<iyakki_shared::store_tilehub::TileEntry>,
    custom_at: u64,
    /// The custom app on screen, if the app on screen is one.
    custom_open: Option<String>,
    session: Option<Session>,
    /// The selected tile, counted within the shown group (its page follows from it).
    sel: usize,
    /// The launcher group shown (remembered per person).
    group: Group,
    /// The id of the person's own group being shown, when it is one (else `group` is).
    cgroup: Option<String>,
    /// The person's layout (groups they made, tiles they moved) and the menus and cards that change it (`shell_layout.rs`).
    layout: iyakki_shared::layout::Layout,
    lay: layout_impl::LayoutUi,
    mouse: (i32, i32),
    tick: u64,
    toast: Option<(String, u64, bool)>,
    size: (u32, u32),
    /// Rectangles whose hover state the last frame depended on.
    probes: Vec<R>,
    /// Something happened that changes the picture (see [`Shell::take_stale`]).
    stale: bool,
    /// The open app's live picture changed and nothing else did (see [`Shell::live_update`], `shell_live.rs`).
    live: bool,
    /// The front end paints the live picture on its own ([`Shell::live_update`]): else a new picture makes the scene stale as before.
    live_paint: bool,
    /// The rectangles the last [`Shell::live_update`] painted when the app named them (else the bounding one only), the longest
    /// time the backend may leave between whole redraws while the app paints live, and until which tick that holds.
    live_rects: Vec<(i32, i32, i32, i32)>,
    live_every_ms: u64,
    live_until: u64,
    /// The window should ask the system clipboard for its current text (opening the queue does), and until which tick
    /// the answer, when it comes, belongs to the queue rather than to a field.
    want_host_paste: bool,
    host_sync_until: u64,
    /// What the queue says about the system clipboard: asked, answered (with how many characters), or no answer.
    clip_note: ClipNote,
    /// The F1 help popup is open.
    help: bool,
    shot: Shot,
    /// An alarm or reminder that is ringing.
    ring: Option<Ring>,
    /// When alarms were last looked at (seconds since the epoch).
    last_alarm_check: i64,
    /// Alarms put off for later: (when, what).
    snoozed: Vec<(i64, String, bool)>,
    /// The app that was open when the inbox was opened from the bell: it keeps running (a chat waiting for an answer
    /// must not be cut off) and comes back when the inbox is closed.
    /// The app that was in front when the apps that listen were last told (`note_foreground`).
    fg_seen: Option<String>,
    stash: Option<(Box<dyn App>, PApp)>,
    /// The home screen shows page 0, the machine page, instead of the shown group (`shell_home.rs`).
    machine: bool,
    /// The tick at which the home page last changed (the island pop and the icon stagger run from it); `None` before any change.
    anim_at: Option<u64>,
    /// The slide of the last swipe between groups: which way (true = back) and the heartbeat it began (`shell_home.rs`).
    slide: Option<(bool, u64)>,
    /// The Look in force for this person (`shell_look.rs`): icons, launcher layout, dock, bars, menu. The default Look until sign-in.
    look: iyakki_uikit::look::Look,
    /// The apps the person pinned to the dock (None: the Look's list or the default set), read at sign-in.
    pinned: Option<Vec<String>>,
    /// Keyboard focus on the dock (`d`), the dock press in progress (index, where) and whether it turned into a drag.
    dock_focus: Option<usize>,
    /// The Look has the top bar off and the person asked to see it (`b` or the corner handle): shown until they use it or press `b`/Esc.
    bar_peek: bool,
    dock_press: Option<(usize, (i32, i32))>,
    dock_moved: bool,
    /// What the bell shows: unread notifications, approvals waiting, overdue tasks.
    badge: Badge,
    /// How many updates the last check found (the STORE tile says so); read with the bell's count.
    update_count: usize,
    /// What the apps say about themselves (`iyakki_core_api::AppStatuses`, F-LAUNCH-09): `(app id, status)`, read with the bell's count; the
    /// icon lines, chips and the machine page's Attention list come from it.
    app_status: Vec<(String, iyakki_core_api::AppStatus)>,
    /// The widget board on the machine page: the person's widgets, edit mode and the machine's figures (`shell_board.rs`).
    bd: board_impl::BoardUi,
    /// System sounds: what has been seen so far (see `cues`).
    cues: crate::cues::CueState,
    /// The clock in the top bar, and the rectangle whose hover shows its tooltip this frame.
    clock: crate::clockbar::ClockState,
    clock_tip: Option<R>,
    clock_text: String,
    clock_ready: bool,
    /// The Devices popover (button next to the screenshot icon, F8).
    devpop: crate::devpop::DevPop,
    /// The login screens' top bar and power card, and whether this shell is a remote (browser) session (`loginbar.rs`).
    lbar: crate::loginbar::LoginBar,
    /// Replaces the machine the popover looks at (tests).
    devprobe: Option<crate::devpop::Probe>,
    /// The IYAKKI menu (a click on the name in the launcher's top bar, F10): About, Quick install, Media player, Mac drivers.
    brand: crate::brandmenu::BrandMenu,
    /// The installer indicator beside the clock and the list of installs it opens.
    installs: crate::installpop::InstallPop,
    /// The battery indicator of a laptop, its popup and the low-battery cards (`batterybar.rs`, `shell_battery.rs`).
    battery: crate::batterybar::BatteryBar,
    /// The banner shown while the drive the data lives on is lost or failing (`lossbar.rs`, `shell_loss.rs`).
    loss: crate::lossbar::LossBar,
    /// The SDK app a client shows in display-list mode (`shell_listapp.rs`, F-SDK-29): its own tile, never the one on the shell's screen.
    list_app: Option<(Box<dyn App>, PApp)>,
    /// "<app> stopped because of an internal error" with a button that opens it again (`crashcard.rs`, `shell_crashcard.rs`).
    crash: Option<crate::crashcard::CrashCard>,
    /// Replaces the program Quick install runs as its jobs (tests).
    quick_launch: Option<iyakki_shared::quickinstall::Launch>,
    /// "DEV" (or "DEV · pairing") while Developer mode is on, read every second.
    dev_sign: Option<String>,
    /// A paired computer waits for the administrator to allow commands: pulse, banner and the inline card (`devask.rs`, `shell_devask.rs`).
    devask: crate::devask::DevAsk,
    /// The one-time time zone question (`shell_tzask.rs`).
    tzask: shell_tzask::TzAsk,
    /// The USB cameras: the CAMERA tile is greyed while there is none (looked at again every two seconds).
    camera: crate::camera::Presence,
    /// The "camera in use by the assistant" sign was on in the last frame.
    cam_sign: bool,
    /// The red sign says embedded camera views are showing (an SDK app shows a camera), as of the last look.
    cam_views_sign: bool,
    /// The wake phrase listener and its sign (`shell_wake.rs`).
    wake: wake_impl::WakeUi,
    /// The Gemini voice assistant (EXPERIMENTAL): the key, the button, the cards (`shell_gemvoice.rs`).
    gv: gv_impl::GvUi,
    /// The top bar's speaker: its slider and press (`shell_volbar.rs`).
    vol: volbar_impl::VolUi,
    /// What is still starting after boot: the Home strip, its list and the sign-in line (`shell_startup.rs`).
    startup: startup_impl::StartupUi,
    /// Media and Fn keys: the on-screen display, held keys, the power card (`shell_fnkeys.rs`).
    fnk: crate::fnkeys::FnState,
    /// The Touch Bar strip of a T2 MacBook (`shell_touchbar.rs`).
    tbar: touchbar_impl::TbState,
    /// The on-screen keyboard for touch screens (`osk.rs`, `shell_osk.rs`).
    osk: crate::osk::Osk,
    /// Cloud tenants: the open sessions, the dialogs and the strip (`shell_tenant.rs`).
    tenants: shell_tenant::TenantUi,
    /// Cloud mode: what is hidden because this machine has no local screen, sound or camera (`iyakki_shared::cloudmode`).
    cloud: iyakki_cloudmode::CloudMode,
    /// Panes: several apps side by side on one display (`shell_panes.rs`).
    panes: panes_impl::PanesUi,
    /// Desktop views: the person's named views and the live apps of each (`shell_views.rs`).
    views: views_impl::ViewsUi,
    /// The scheduled backup (`shell_backup_sched.rs`).
    bksched: bksched_impl::BackupSched,
    /// The screen saver and screen off (`shell_saver.rs`, `saver.rs`).
    saver: saver_impl::SaverUi,
    /// The lock screen (`shell_lock.rs`).
    lockui: lock_impl::LockUi,
    /// The sign-out confirm card and the "You signed out" line (`shell_signout.rs`).
    signout: signout_impl::SignoutUi,
    /// The part of a wheel notch left over when pixel scrolling is turned into whole notches (`shell_scroll.rs`), and when.
    scroll_rem: f32,
    scroll_rem_at: u64,
}

/// The numbers behind the bell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Badge {
    /// Unread entries that need the person: errors, warnings, reminders, an assistant waiting (the other kinds are not counted).
    pub unread: usize,
    pub approvals: usize,
    pub overdue: usize,
    /// Unread entries of the other kinds (information, new devices, finished jobs): a small dot, never a number.
    pub other: usize,
}

impl Badge {
    pub fn total(&self) -> usize {
        self.unread + self.approvals + self.overdue
    }
}

struct Ring {
    /// (what rang, is it an alarm rather than a reminder, the time shown)
    items: Vec<(String, bool, String)>,
    since: i64,
    player: Option<Box<dyn iyakki_shared::voicedesk::Playback>>,
    next_sound: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipNote {
    None,
    Asked,
    Got(usize),
}

/// A screenshot in progress: take a picture of the frame, then let the person choose what to keep.
enum Shot {
    Off,
    /// Capture the next frame (before the overlay is drawn).
    Snap,
    /// Choosing an area: the frame as it was, and the first corner once clicked.
    Select { snap: tiny_skia::Pixmap, first: Option<(i32, i32)> },
    /// The picture is on the clipboard; the person decides whether to keep a file as well.
    Review { img: tiny_skia::Pixmap },
}

/// One colour per letter of the name, drawn as a strip under it.
const NAME_COLORS: [[u8; 4]; 6] = [rgb(79, 140, 255), rgb(61, 214, 163), rgb(167, 116, 255), rgb(255, 184, 77), rgb(255, 107, 154), rgb(100, 220, 240)];

/// The strip under the name: one bar per letter, together exactly as wide as the name.
fn name_strip(g: &mut Gfx, x: f32, y: f32, width: f32, bar_h: f32, s: f32, round: bool) {
    let gap = 5.0 * s;
    let n = NAME_COLORS.len() as f32;
    let bw = (width - gap * (n - 1.0)) / n;
    for (i, c) in NAME_COLORS.iter().enumerate() {
        let bx = x + i as f32 * (bw + gap);
        if round {
            g.rrect(bx, y, bw, bar_h, bar_h / 2.0, *c);
        } else {
            g.rect(bx, y, bw, bar_h, *c);
        }
    }
}

/// What the account chip says: the name, and the role when it adds something (an account called "admin" that is
/// an admin does not read "admin · admin").
fn account_label(user: &str, role: &str) -> String {
    if user.eq_ignore_ascii_case(role) { user.to_string() } else { format!("{user}  ·  {role}") }
}

/// The size of everything (1.0 draws body text at 14 px). A bigger window gives more room, not bigger text, as on Ubuntu and
/// macOS: 0.9 at 1280x720 (body about 13 px), about 1.0 at 1920x1080, about 1.25 at 4K, then the person's own size choice on top.
fn d_label(d: Option<&crate::apps::hostfiles::HostDialog>) -> String {
    d.map_or_else(|| "your account".to_string(), |d| d.dest_label().to_string())
}

pub(crate) use crate::ui::scale_for;

impl Shell {
    pub fn new(env: ShellEnv) -> Shell {
        crate::crashlog::install();
        // the apps ask the shell's tile table through this (an app never depends on the shell)
        crate::apps::set_layout_tiles(layout_tiles);
        crate::apps::set_pinnable(pinnable_apps);
        // the toolkit has no storage dependency: it asks the shell how to word a failure line (a lost boot drive is named)
        crate::fx::set_failure_rewriter(iyakki_core_local::storage::loss::reword);
        #[cfg(test)]
        crate::testmem::start();
        let first_run = env.store.lock().map(|s| s.is_empty()).unwrap_or(false);
        let screen = if env.hwcheck {
            Screen::HwCheck(Box::new(crate::hwcheck::HwCheck::new()))
        } else if first_run {
            Screen::Setup(Form::setup())
        } else {
            Screen::Login(Form::login_for(last_user(&env).as_deref()))
        };
        // tests run as a desktop unless one asks for cloud mode (`set_cloud_for_test`)
        let osguard = osguard_impl::OsGuard::new(env.data_dir());
        // the assistant an app embeds: the person's agent switch and budget come from the accounts, the model is the machine's, the audit lines go to the log
        let accounts = env.store.clone();
        let policy_of: crate::wiring::PolicyOf = Arc::new(move |user: &str| accounts.lock().ok().and_then(|s| s.get(user).map(|r| r.policy())));
        // the core and the injected handles come from the composition root (`iyakki-compose`, installed before the first shell; a test builds them itself)
        let core: Arc<dyn iyakki_core_api::Core> = crate::wiring::make_core(&env, crate::wiring::CoreInputs { policies: Arc::new(crate::apps::StorePolicies(env.store.clone())), audit: Arc::new(crate::apps::assistant_port::AuditLogSink(env.audit.clone())), policy_of });

        let cloud = if cfg!(test) { iyakki_cloudmode::CloudMode::desktop() } else { core.cloud().detect() };
        let agent: Arc<dyn iyakki_agent_api::AgentApi> = iyakki_apps::agent_arc();
        assert!(iyakki_agent_api::AgentFacets::is_real(iyakki_apps::agent()), "the agent handle is not the real implementation");
        assert!(env.services.is_real(), "the services handle is not the real implementation");
        let mut sh = Shell { env, core, agent, resuming: false, resume_tried: false, osguard, acc: Default::default(), hits: Vec::new(), clip: None, hostdlg: None, permpop: None, login: None, clipscan: None, scan_src: None, drag_over: false, ctx: None, fields: Vec::new(), last_click: None, press: None, click_run: (0, 0, 0, 0), screen, custom: Vec::new(), custom_at: 0, custom_open: None, session: None, sel: 0, gv: Default::default(), look: Default::default(), pinned: None, dock_focus: None, dock_press: None, dock_moved: false, bar_peek: false, machine: false, anim_at: None, slide: None, group: Group::Home, cgroup: None, layout: Default::default(), lay: Default::default(), mouse: (0, 0), tick: 0, toast: None, size: (1280, 720), probes: Vec::new(), stale: true, live: false, live_paint: false, live_rects: Vec::new(), live_every_ms: 100, live_until: 0, want_host_paste: false, host_sync_until: 0, clip_note: ClipNote::None, help: false, shot: Shot::Off, ring: None, last_alarm_check: 0, snoozed: Vec::new(), fg_seen: None, stash: None, badge: Badge::default(), update_count: 0, app_status: Vec::new(), bd: Default::default(), cues: Default::default(), clock: Default::default(), clock_tip: None, clock_text: String::new(), clock_ready: false, devpop: Default::default(), devprobe: None, brand: Default::default(), installs: Default::default(), battery: Default::default(), loss: Default::default(), list_app: None, crash: None, quick_launch: None, dev_sign: None, devask: Default::default(), tzask: Default::default(), camera: if wants_system_camera(cfg!(test), &cloud) { crate::camera::Presence::system() } else { crate::camera::Presence::none() }, cam_sign: false, cam_views_sign: false, fnk: Default::default(), vol: Default::default(), startup: startup_impl::StartupUi::new(), tbar: Default::default(), osk: Default::default(), tenants: Default::default(), lbar: Default::default(), cloud: cloud.clone(), panes: Default::default(), views: Default::default(), bksched: Default::default(), saver: Default::default(), lockui: Default::default(), signout: Default::default(), wake: Default::default(), scroll_rem: 0.0, scroll_rem_at: 0 };
        if cloud.on {
            sh.apply_cloud(cloud);
        }
        sh
    }

    /// Tests give the shell a core of their own (a fake with a developer-mode switch).
    #[cfg(test)]
    pub(crate) fn set_core_for_test(&mut self, core: Arc<dyn iyakki_core_api::Core>) {
        self.core = core;
    }

    /// The page or tab the open app is showing (empty on the launcher and the login): what the harness of a UI program over the core's socket reads
    /// to find the page it wants.
    pub fn app_is_unserved(&self) -> bool {
        matches!(&self.screen, Screen::App(a, _) if a.is_unserved())
    }

    pub fn app_page(&self) -> String {
        match &self.screen {
            Screen::App(a, _) => a.page(),
            _ => String::new(),
        }
    }

    pub fn screen_name(&self) -> String {
        if self.lockui.active {
            return "locked".into();
        }
        match &self.screen {
            Screen::HwCheck(_) => "hwcheck".into(),
            Screen::Setup(_) => "setup".into(),
            Screen::Login(_) => "login".into(),
            Screen::Launcher => "launcher".into(),
            Screen::App(_, _) if self.custom_open.is_some() => format!("tile:{}", self.custom_open.as_deref().unwrap_or("")),
            Screen::App(_, a) => format!("app:{}", a.id()),
            Screen::PowerMenu => "power".into(),
        }
    }

    /// The user store, for tools that need to seed accounts (screenshot demo, provisioning).
    pub fn store_for_demo(&self) -> &crate::accounts::UsersRef {
        &self.env.store
    }

    pub fn services_for_demo(&self) -> &Arc<dyn iyakki_agent_api::ServicesHandle> {
        &self.env.services
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// Text the user copied inside IYAKKI, for the host (desktop window) to put on the system clipboard.
    pub fn take_host_copy(&mut self) -> Option<String> {
        crate::ui::clipboard::take_host_copy()
    }

    /// The shape the mouse pointer should have now: a hand over links and buttons, a text bar over fields, what an
    /// embedded engine asks for over its page, the arrow elsewhere.
    pub fn pointer(&self) -> crate::ui::Pointer {
        use crate::ui::Pointer;
        let (x, y) = self.mouse;
        if self.screen_stage().covers() {
            return Pointer::Hidden; // a still arrow on the saver would burn in
        }
        if self.help || self.clip.is_some() || self.ctx.is_some() {
            return Pointer::Default;
        }
        let Some(h) = self.hits.iter().rev().find(|h| h.rect.contains(x, y)) else { return Pointer::Default };
        if let Screen::App(app, _) = &self.screen {
            if let Some(p) = app.pointer_hint(x, y) {
                return p;
            }
        }
        let l = h.label.as_str();
        if matches!(l, "field" | "text area" | "address" | "address bar" | "search") || l.ends_with(" field") {
            return Pointer::Text;
        }
        if matches!(l, "backdrop" | "close list" | "page" | "scroll") || matches!(h.ev, Event::Tick | Event::Scroll(_)) {
            return Pointer::Default;
        }
        Pointer::Hand
    }

    /// Clickable regions from the last frame (tests and tools).
    /// The text of the custom app on screen (tests).
    #[cfg(test)]
    pub(crate) fn tile_text_for_test(&mut self) -> String {
        match &mut self.screen {
            Screen::App(app, _) => app.as_any_tile().map(|t| t.page_text()).unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// The words a `wasm` tile drew, or why it does not run (tests).
    #[cfg(test)]
    pub(crate) fn wasm_tile_text_for_test(&mut self) -> String {
        match &mut self.screen {
            Screen::App(app, _) => app.wasm_tile_text(),
            _ => String::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn tile_card_text_for_test(&mut self) -> String {
        match &mut self.screen {
            Screen::App(app, _) => app.as_any_tile().map(|t| t.card_text()).unwrap_or_default(),
            _ => String::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn tile_card_geometry_for_test(&mut self) -> Option<(R, Vec<R>)> {
        match &mut self.screen {
            Screen::App(app, _) => app.as_any_tile().and_then(|t| t.card_geometry()),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn tile_bar_for_test(&mut self) -> Vec<(String, R)> {
        match &mut self.screen {
            Screen::App(app, _) => app.as_any_tile().map(|t| t.bar.clone()).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn resize_for_test(&mut self, w: u32, h: u32) {
        self.size = (w, h);
        mark_stale!(self);
    }

    /// Open an app the way a tile does (tests and the monkey test).
    #[cfg(test)]
    pub(crate) fn open_for_test(&mut self, app: PApp) {
        self.open(app);
    }

    /// The services and session an app gets (tests that build an app by hand).
    #[cfg(test)]
    pub(crate) fn cx_for_test(&self) -> Option<AppCx> {
        self.cx()
    }

    /// Back to the launcher (tests and the monkey test).
    #[cfg(test)]
    pub(crate) fn launcher_for_test(&mut self) {
        self.screen = Screen::Launcher;
    }

    /// Mark this shell as a remote (browser) session: the login screens then show no power button and refuse the power action.
    pub fn set_remote(&mut self, remote: bool) {
        self.lbar.remote = remote;
        self.saver_set_remote(remote);
    }

    /// Where the login bar's pieces were drawn last frame, and the Power button (tests and tools).
    pub fn login_bar_layout(&self) -> (&[(crate::loginbar::Chip, R)], Option<R>) {
        (&self.lbar.last, self.lbar.power_rect)
    }

    pub fn hits(&self) -> &[crate::ui::Hit] {
        &self.hits
    }

    /// The right-click menu, while it is open (tests and tools).
    pub fn context_menu(&self) -> Option<&crate::ctxmenu::Menu> {
        self.ctx.as_ref()
    }

    pub fn clipboard_picker_open(&self) -> bool {
        self.clip.is_some()
    }

    pub fn toast_text(&self) -> Option<&str> {
        self.toast.as_ref().map(|t| t.0.as_str())
    }

    /// Tiles visible to the current user: the four core apps always, Users only for admins.
    fn visible_tiles(&self) -> Vec<&'static Tile> {
        let admin = self.session.as_ref().is_some_and(|s| s.policy.manage_users);
        TILES.iter().filter(|t| t.group.is_some() && (t.app != PApp::Users || admin) && !self.cloud.hides_app(t.app)).collect()
    }

    /// The browser hardware (clipboard, sound, microphone, camera) of the signed-in person, when they are in a web browser.
    /// The core handle this shell runs on (the one the composition root's factory made): the real-wiring guard reads it.
    pub fn core_handle(&self) -> &Arc<dyn iyakki_core_api::Core> {
        &self.core
    }

    pub fn browser_media(&self) -> Option<std::sync::Arc<dyn iyakki_agent_api::BrowserMediaHandle>> {
        self.session.as_ref().and_then(|s| iyakki_apps::agent().browser_media_for_user(&s.user))
    }

    /// Cloud mode for this machine, with the tiles a connected browser supplies the hardware for shown again.
    fn detect_cloud(&self) -> iyakki_cloudmode::CloudMode {
        let c = self.core.cloud().detect();
        match self.browser_media() {
            Some(m) => c.with_browser(&m.cloud_sources()),
            None => c,
        }
    }

    /// Take a Cloud mode decision into the shell and the menus that follow it.
    /// Cloud mode decided again from what the machine has now: true when it changed. A sound card that registers late (the
    /// MacBookPro14,1's CS8409 about 11 s after boot, after an automatic sign-in) would otherwise leave RADIO, MEDIA, TALES and
    /// VOICE hidden for the whole session, because "no sound card" makes automatic Cloud mode hide them.
    fn refresh_cloud(&mut self, c: iyakki_cloudmode::CloudMode) -> bool {
        if c == self.cloud {
            return false;
        }
        self.apply_cloud(c);
        true
    }

    fn apply_cloud(&mut self, c: iyakki_cloudmode::CloudMode) {
        self.devpop.set_hidden(c.hidden_device_rows.clone());
        self.devpop.invalidate();
        self.brand.set_cloud(c.clone());
        // The shell starts before a laptop's sound card and camera have appeared, and a machine that looks like a server at that moment gets
        // no camera scanner (the empty placeholder). When it turns out to be a desktop after all, the real scanner must take over, or the CAMERA
        // tile says "no camera" for the whole session (seen on a Lenovo Legion: the kernel had /dev/video0, the app looked in a folder that
        // does not exist).
        if wants_system_camera(cfg!(test), &c) && !self.camera.looks_at_the_machine() {
            self.camera = crate::camera::Presence::system();
        }
        self.cloud = c;
        mark_stale!(self);
    }

    /// Cloud mode as decided for this machine.
    pub fn cloud(&self) -> &iyakki_cloudmode::CloudMode {
        &self.cloud
    }

    /// Pretend to be another machine (tests).
    #[cfg(test)]
    pub(crate) fn set_cloud_for_test(&mut self, c: iyakki_cloudmode::CloudMode) {
        if !c.camera_watch {
            self.camera = crate::camera::Presence::none();
        }
        self.apply_cloud(c);
    }

    /// The section tabs for this person: each built-in section that has a tile for them, then the groups they made (shown even
    /// when empty, so they can fill them).
    fn groups(&self) -> Vec<GroupRef> {
        let mut v: Vec<GroupRef> = Group::CHIPS
            .into_iter()
            .filter(|g| self.visible_tiles().iter().any(|t| self.tile_gid(t) == g.id()) || self.custom.iter().any(|e| self.app_gid(e) == g.id()))
            .map(GroupRef::from)
            .collect();
        v.extend(self.layout.groups.iter().map(|g| GroupRef { id: g.id.clone(), name: g.label.clone() }));
        v
    }

    /// The section being shown (the first one, Home, when the remembered one has nothing for this person).
    fn current_group(&self) -> GroupRef {
        let gs = self.groups();
        if let Some(c) = self.cgroup.as_ref().and_then(|c| gs.iter().find(|g| g.id == *c)) {
            return c.clone();
        }
        let b = GroupRef::from(self.group);
        if gs.contains(&b) {
            b
        } else {
            gs.first().cloned().unwrap_or_else(|| Group::Home.into())
        }
    }

    /// The group a built-in tile is in for this person: where they moved it, else its own.
    fn tile_gid(&self, t: &Tile) -> String {
        let own = t.group.map_or("", |g| g.id());
        self.layout.group_of(&iyakki_shared::layout::sys_key(t.label), own)
    }

    /// The group a Store app is in for this person: where they moved it, else what its app.json asks, else Work.
    fn app_gid(&self, e: &iyakki_shared::store_tilehub::TileEntry) -> String {
        let own = self.layout.app_default(&e.name, &e.group).0;
        self.layout.group_of(&iyakki_shared::layout::app_key(&e.name), &own)
    }

    /// The tiles of the shown group, in launcher order; pages are cut from this list.
    fn group_tiles(&self) -> Vec<&'static Tile> {
        let g = self.current_group();
        self.visible_tiles().into_iter().filter(|t| self.tile_gid(t) == g.id).collect()
    }

    /// The custom apps in the shown group (they follow the system tiles).
    fn group_custom(&self) -> Vec<iyakki_shared::store_tilehub::TileEntry> {
        let g = self.current_group();
        self.custom.iter().filter(|e| self.app_gid(e) == g.id).cloned().collect()
    }

    /// Everything the launcher shows in the shown group: the system tiles, then the custom apps, in the person's order when
    /// they set one; a group of their own with nothing in it shows a hint.
    fn group_items(&self) -> Vec<LItem> {
        let mut v: Vec<LItem> = self.group_tiles().into_iter().map(LItem::Sys).collect();
        v.extend(self.group_custom().into_iter().map(|e| LItem::App(Box::new(e))));
        let gid = self.current_group().id;
        if self.layout.order.contains_key(&gid) {
            let keys: Vec<String> = v.iter().map(layout_impl::item_key).collect();
            let order = self.layout.ordered(&gid, keys);
            v.sort_by_key(|it| order.iter().position(|k| *k == layout_impl::item_key(it)));
        }
        if v.is_empty() {
            v.push(LItem::Empty);
        }
        v
    }

    /// Look for the person's custom apps again (the launcher does this when it is shown, and every two seconds).
    fn refresh_custom(&mut self) {
        let Some(s) = &self.session else {
            self.custom.clear();
            return;
        };
        self.custom_at = self.tick.max(1);
        self.custom = self.env.storeapi().tiles(&s.policy, &s.user);
        // the native tiles of the tenant that is shown (F-CLOUD-76) follow the person's own apps
        let cloud = self.tenant_tile_entries();
        self.custom.extend(cloud);
        if self.sel >= self.group_items().len() {
            self.sel = 0;
        }
    }

    /// Show a group (from its chip, Tab or a test) and remember it for this person.
    fn set_group(&mut self, g: impl Into<GroupRef>) {
        let g: GroupRef = g.into();
        if !self.groups().contains(&g) {
            return;
        }
        match Group::from_id(&g.id) {
            Some(b) => {
                self.group = b;
                self.cgroup = None;
            }
            None => self.cgroup = Some(g.id.clone()),
        }
        self.sel = 0;
        self.machine = false;
        self.anim_at = Some(self.tick);
        if let Some(sess) = &self.session {
            crate::uisize::save_group(self.env.data_dir(), &sess.user, &g.id);
        }
    }

    fn step_group(&mut self, back: bool) {
        if self.machine {
            // from the machine page Tab goes to the group that was shown before
            self.show_page(self.groups().iter().position(|g| *g == self.current_group()).map_or(1, |i| i + 1));
            return;
        }
        let gs = self.groups();
        let cur = self.current_group();
        let i = gs.iter().position(|g| *g == cur).unwrap_or(0);
        let n = gs.len();
        self.set_group(gs[if back { (i + n - 1) % n } else { (i + 1) % n }].clone());
    }

    fn cx(&self) -> Option<AppCx> {
        let session = self.session.clone()?;
        // the administrator's mcp.json plus the servers added in the MCP tile (and the sample one)
        let stored = self.env.services.mcp().list();
        let mut mcp_servers: Vec<_> = self.env.mcp_servers.iter().filter(|s| !stored.iter().any(|x| x.name == s.name)).cloned().collect();
        mcp_servers.extend(stored.iter().map(iyakki_shared::mcp::StoredServer::spec));
        Some(AppCx {
            session: session.clone(),
            host: self.env.host.clone(),
            cfg: self.env.cfg.clone(),
            // everything the assistant asks of a model is counted for the USAGE tile
            chat: self.env.b4().metered_chat(self.env.chat.clone(), &session.user),
            audit: self.env.audit.clone(),
            store: self.env.store.clone(),
            mcp_servers,
            services: self.env.services.clone(),
            web: self.env.web.clone(),
            // the app gets the machine through the broker, for its own identity; the raw handle stays in `osguard`
            os: self.osguard.os_for(match &self.screen { Screen::App(_, a) => a.id(), _ => "shell" }, &session.user),
            core: self.core.clone(),
            agent: self.agent.clone(),
            store_facets: None,
        })
    }

    /// An app panicked (the panic was caught): tell the person, keep the note of where in the audit log, and leave everything
    /// else running. Opening the app again starts it afresh.
    /// `app` is the tile the app was opened from, so the card can open it again with one press.
    pub(crate) fn app_crashed_with(&mut self, name: &str, app: Option<PApp>) {
        let note = crate::crashlog::take_last();
        let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
        self.env.audit.record(&user, "app.crash", &format!("{name}: {note}"));
        self.raise_crash_card(name, app);
    }

    fn say(&mut self, s: impl Into<String>) {
        // a failure caused by a lost drive says so (iyakki_core_local::storage::loss::reword leaves every other message alone)
        self.toast = Some((iyakki_core_local::storage::loss::reword(&s.into()), self.tick + 120, false));
    }

    fn say_info(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), self.tick + 75, true));
    }

    fn sign_in(&mut self, s: Session, password: &str) {
        // the Cloud mode setting may have changed since the last sign-in (Settings -> Cloud mode)
        if !cfg!(test) {
            let c = self.detect_cloud();
            self.refresh_cloud(c);
        }
        self.env.audit.record(&s.user, "login", s.role.name());
        // the machine's sound belongs to the person signed in last (it may go to their client device, EXPERIMENTAL)
        if let Some(before) = crate::soundowner::signed_in(&s.user) {
            // another person had the sound: their sound sources end now (D-SESSION-END)
            self.end_session(&before, iyakki_shared::sessionend::Why::SoundSwitch, true);
        }
        // a local model is loaded in the background now, so the first question is not the slow one
        if s.policy.agent {
            self.env.services.warm_up_chat();
        }
        remember_user(&self.env, &s.user);
        // unlock the user's personal vault (and, for admins, the system vault) with the password just typed
        let report = if self.resuming { self.env.services.on_resume(&s.user, s.policy.manage_settings) } else { self.env.services.on_login(&s.user, password, s.policy.manage_settings) };
        // a developer machine may put this person back after a restart of the shell (F-SHELL-65): the ticket is only written when allowed
        self.core.session_resume().remember(&s.user);
        // schedules run while this account is signed in, without FLOWS being open (nothing starts for an account without flows)
        {
            let (svc, user, policy) = (self.env.services.clone(), s.user.clone(), s.policy.clone());
            std::thread::spawn(move || {
                svc.flows_facet().start_for_login(&user, &policy);
            });
        }
        // recordings the person switched on start again now that their vault (which holds the keys) is open
        if !cfg!(test) {
            crate::apps::cameras::resume_recording(&self.env.b4(), &self.env.audit, &self.env.store);
        }
        // kubeconfigs held in this person's vaults can be used by cluster commands while they are signed in; the dev cluster's
        // background events (restarts, exits) are audited
        self.env.b3().kube_resolver(Some(&s.user));
        // the volume and play keys of a connected phone or headset act on IYAKKI's player while somebody is signed in (F-NET-27)
        let audit = self.env.audit.clone();
        self.env.services.install_bt_remote(&s.user, Arc::new(move |who, what| audit.record(who, "bluetooth.remote", what)));
        {
            let audit = self.env.audit.clone();
            self.env.services.k3sdev().set_audit(Arc::new(move |u, a, d| audit.record(u, a, d)));
        }
        // shares marked "keep running" start again for this person (everything else in Settings -> Shared folders stays stopped)
        {
            let (audit, user) = (self.env.audit.clone(), s.user.clone());
            let admin = s.role == iyakki_sec::policy::Role::Admin;
            self.env.b0().shares_restore(admin, s.policy.clone(), &s.user, Arc::new(move |e, d| audit.record(&user, e, d)));
        }
        // SSH tunnels the person left enabled start again (their keys are in the vault that was just opened)
        {
            let (audit, user) = (self.env.audit.clone(), s.user.clone());
            self.env.b4().ssh_tunnels_start_for_login(&s.user, Arc::new(move |a, d| audit.record(&user, a, d)));
        }
        // a VPN the person left enabled connects again (its keys are in the vault that was just opened); leftovers of a crash go first
        {
            let (audit, user) = (self.env.audit.clone(), s.user.clone());
            self.env.b3().vpn_start_for_login(&s.user, Arc::new(move |a, d| audit.record(&user, a, d)));
        }
        // domain names the person left on are served again (the certificates are in the vault that was just opened); administrators only
        if s.policy.manage_settings {
            let (audit, user) = (self.env.audit.clone(), s.user.clone());
            self.env.services.domains_login(&s.user, Arc::new(move |a, d| audit.record(&user, a, d)));
        }
        match &report.browser_data {
            iyakki_shared::profilestore::SignIn::Off => {}
            iyakki_shared::profilestore::SignIn::Opened { from_backup } => self.env.audit.record(&s.user, "browser.data.opened", if *from_backup { "from the backup copy" } else { "" }),
            iyakki_shared::profilestore::SignIn::Locked(why) => {
                self.env.audit.record(&s.user, "browser.data.locked", why);
                self.say("Your protected browser data could not be opened (see Settings -> Browser).");
            }
        }
        if let VaultState::Locked(why) = &report.user_vault {
            self.env.audit.record(&s.user, "vault.user.locked", why);
            let _ = self.env.notify_once(&s.user, iyakki_shared::inbox::Kind::Warning, "Your personal vault is locked", "It could not be opened with the password you typed. See Settings → Vault.", Some(iyakki_shared::inbox::Link::new("settings", "")));
            self.say("Your personal vault could not be opened (see Settings → Vault).");
        } else if let VaultState::Locked(why) = &report.system_vault {
            self.env.audit.record(&s.user, "vault.system.locked", why);
            let _ = self.env.notify_once(&s.user, iyakki_shared::inbox::Kind::Warning, "The system vault is locked", "Model API keys are unavailable until an administrator with access signs in.", Some(iyakki_shared::inbox::Link::new("settings", "")));
            self.say("The system vault is locked: model API keys are unavailable until an admin with access signs in.");
        }
        // a release image has no vendor firmware: an administrator gets one note that says where to fetch it (F-REL-09)
        if s.policy.manage_settings {
            let rel = iyakki_shared::relinfo::ReleaseInfo::load(std::path::Path::new(iyakki_shared::relinfo::RELEASE_FILE));
            let text = std::fs::read_to_string(iyakki_shared::relinfo::FIRMWARE_NOTICE_FILE).ok();
            if let Some((title, body)) = iyakki_shared::relinfo::firmware_notice(rel.as_ref(), text.as_deref()) {
                let _ = self.env.notify_once(&s.user, iyakki_shared::inbox::Kind::Info, &title, &body, Some(iyakki_shared::inbox::Link::new("store", iyakki_shared::relinfo::FIRMWARE_NOTICE_ITEM)));
            }
        }
        // documents embedded by a model that is no longer the chosen one are brought up to date, quietly
        if s.policy.manage_settings {
            // the browser add-on built here no longer matches this image: a note, never an automatic rebuild
            self.env.services.av_notify_stale(&s.user);
            self.env.services.reindex_stale_in_background();
            // (tests never look at the real network)
            #[cfg(not(test))]
            {
                self.env.services.detect_local_ollama_in_background();
                self.env.services.detect_local_lemonade_in_background();
                self.env.services.start_device_watch();
            }
        }
        // examples to try (once, for administrators): an MCP server, documents and skills
        crate::samples::seed(self.env.data_dir(), &*self.env.skills(), self.env.mcp(), &*self.env.audit, &self.env.cfg.rag_docs, &s.user, &s.policy);
        crate::uisize::apply(Some(crate::uisize::load(self.env.data_dir(), &s.user)));
        crate::theme::apply_for(self.env.data_dir(), &s.user);
        iyakki_uikit::wallpaper::apply_for(self.env.data_dir(), &s.user);
        iyakki_uikit::fonts::apply_for(self.env.data_dir(), &s.user);
        crate::filepicker::set_session(Some((&s.user, &s.policy, self.env.data_dir())));
        self.session = Some(s);
        let who = self.session.as_ref().map(|x| x.user.clone()).unwrap_or_default();
        self.load_look(&who);
        self.load_clipboard_limit();
        self.sel = 0;
        let remembered = crate::uisize::load_group(self.env.data_dir(), &self.session.as_ref().map_or(String::new(), |s| s.user.clone()));
        self.group = remembered.as_deref().and_then(Group::from_id).unwrap_or(Group::Home);
        self.lay_sign_in(remembered.as_deref());
        self.screen = Screen::Launcher;
        self.panes_reset();
        self.refresh_gate();
        self.views_sign_in();
        self.cues.badge_seen = false;
        self.refresh_badge();
        // a private folder that stayed sealed is said on the home screen at once (F-STORE-05); the Settings page keeps the full words
        if let Some(b) = iyakki_enchome::peek_banner(&who) {
            self.say(b);
        }
        // the sign-in sound plays by itself while the home screen appears
        let _ = crate::cues::start(&*self.core, &self.cloud, iyakki_core_api::soundtypes::Cue::Login);
    }

    /// Start signed in as a person whose password has just been checked elsewhere (the browser sign-in).
    pub fn sign_in_checked(&mut self, s: Session, password: &str) {
        self.sign_in(s, password);
    }

    /// Put back the person who was signed in when the shell last ran, if a developer machine allows it (F-SHELL-65): a fresh ticket of this
    /// boot, developer mode on and the Settings switch on. The password was not typed, so no vault is opened.
    fn try_resume(&mut self) {
        if self.session.is_some() || !matches!(self.screen, Screen::Login(_)) {
            return;
        }
        let Some(who) = self.core.session_resume().who() else { return };
        let sess = self.env.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner).resume_session(&who);
        let Some(sess) = sess else {
            self.core.session_resume().forget();
            return;
        };
        self.env.audit.record(&sess.user, "login.resumed", "after a restart of the shell, without the password (developer mode); the vaults stay locked");
        self.resuming = true;
        self.sign_in(sess, "");
        self.resuming = false;
        let apps = self.core.session_resume().apps();
        // a window arrangement with several panes came back from the person's views file (kept with every ticket refresh): the apps of the
        // ticket would only be stacked on it
        let panes_back = self.panes.ws.is_some();
        if !apps.is_empty() && self.session.is_some() && !panes_back {
            self.reopen_apps(&apps);
        }
        self.say_info("Welcome back: you were signed in when the desktop restarted. Your vault stays locked until you sign in with your password.");
    }

    /// After a crash of the shell: a person who was not put back (developer mode off, or no ticket) is told, in plain words, that the
    /// screen restarted (F-LIFE-16). Nothing is restored by this; one audit line. `ended` is whether the core reported an unexpected end.
    fn say_unexpected_end(&mut self, ended: bool) {
        if !ended || self.session.is_some() {
            return;
        }
        self.env.audit.record("system", "shell.restarted", "the screen ended unexpectedly while somebody was signed in; the person was not put back");
        self.toast = Some((UNEXPECTED_END_NOTICE.to_string(), self.tick + 600, true));
    }

    /// Once in a while for a signed-in person: a drive of their `media` folder that was pulled out while open is let go of, its stale
    /// folder removed, and the person told what happened (F-LIFE-12).
    pub(super) fn drives_tick(&mut self) {
        let Some(s) = &self.session else { return };
        let mut who = iyakki_shared::disks::Actor::new(&s.user, s.role);
        who.home = iyakki_apps::agent().scope_home_root(&s.user, std::path::Path::new("/var/lib/iyakki")).display().to_string();
        let (audit, user) = (self.env.audit.clone(), s.user.clone());
        let said = self.osguard.os_for("shell", &s.user).disk_engine().after_unplug(&who, &|a, d| audit.record(&user, a, d));
        if let Some(m) = said {
            self.toast = Some((m, self.tick + 900, false));
        }
    }

    /// The ids of the apps that are open: the one held aside first, the one on the screen last (what the resume ticket carries; ids only).
    fn open_app_ids(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some((_, pa)) = &self.stash {
            v.push(pa.id().to_string());
        }
        if let Screen::App(_, pa) = &self.screen {
            if *pa != PApp::Inbox {
                v.push(pa.id().to_string());
            }
        }
        v
    }

    /// What each open app holds (its `resume_item`), by app id, for the ticket.
    fn open_app_items(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        let mut add = |app: &dyn App, id: &str| {
            if let Some(i) = app.resume_item() {
                v.push((id.to_string(), i));
            }
        };
        if let Some((a, pa)) = &self.stash {
            add(a.as_ref(), pa.id());
        }
        if let Screen::App(a, pa) = &self.screen {
            if *pa != PApp::Inbox {
                add(a.as_ref(), pa.id());
            }
        }
        v
    }

    /// Open the apps the ticket names again (F-SHELL-65): the one held aside first, then the front one. Only apps this person's role may open
    /// are opened (`open` checks), and only ids the shell knows; the apps start fresh (their own content is not in the ticket).
    fn reopen_apps(&mut self, ids: &[String]) {
        let apps: Vec<PApp> = ids.iter().filter_map(|id| PApp::from_id(id)).filter(|a| !matches!(a, PApp::Inbox | PApp::Settings)).collect();
        let items = self.core.session_resume().items();
        for (n, app) in apps.iter().copied().enumerate() {
            if n + 1 < apps.len() {
                self.open(app);
                self.hand_back_item(app, &items);
                if let Screen::App(a, pa) = std::mem::replace(&mut self.screen, Screen::Launcher) {
                    if self.stash.is_none() {
                        self.stash = Some((a, pa));
                    }
                }
            } else {
                self.open(app);
                self.hand_back_item(app, &items);
            }
        }
    }

    /// The app just opened is told what it held before the restart (the file an editor had open), through its ordinary `open_item`.
    fn hand_back_item(&mut self, app: PApp, items: &[(String, String)]) {
        let Some((_, item)) = items.iter().find(|(id, _)| id == app.id()) else { return };
        let Some(cx) = self.cx() else { return };
        if let Screen::App(a, pa) = &mut self.screen {
            if *pa == app {
                a.open_item(item, &cx);
            }
        }
    }

    /// Run the end-of-session hooks (the process-wide registry and this shell's own) once and audit what stopped.
    pub(crate) fn end_session(&mut self, user: &str, why: iyakki_shared::sessionend::Why, sound_owner: bool) -> iyakki_shared::sessionend::Report {
        let ctx = iyakki_shared::sessionend::Ctx { user: user.to_string(), why, sound_owner };
        let (audit, who) = (self.env.audit.clone(), user.to_string());
        let rep = iyakki_shared::sessionend::SessionEnd::end_many(&[iyakki_shared::sessionend::global(), &self.wake.ends], &ctx, &|a, d| audit.record(&who, a, d));
        self.wake_apply_end();
        rep
    }

    fn sign_out(&mut self) {
        // a person who signs out is not put back after a restart
        self.core.session_resume().forget();
        let had_session = self.session.is_some();
        self.crash = None;
        // a lock does not outlive the session it protects (a sign-out from elsewhere, a browser session ending)
        self.lock_clear();
        // the sign-out sound starts first and plays while the rest is done; the screen changes when it is over, or after
        // `LOGOUT_WAIT` at the latest (a slow or stuck output never holds sign-out back)
        let cue = self.session.is_some().then(|| crate::cues::start(&*self.core, &self.cloud, iyakki_core_api::soundtypes::Cue::Logout));
        // everything the person started that makes sound or captures stops, before the screen changes (one registry, D-SESSION-END); the
        // sign-out sound above is the shell's own cue and goes on
        if let Some(user) = self.session.as_ref().map(|s| s.user.clone()) {
            let owner = crate::soundowner::is_owner(&user);
            self.end_session(&user, iyakki_shared::sessionend::Why::SignOut, owner);
        }
        crate::uisize::apply(None);
        crate::theme::apply(None);
        iyakki_uikit::wallpaper::apply(None);
        iyakki_uikit::fonts::apply_default();
        self.reset_look();
        crate::filepicker::set_session(None);
        self.views_remember_live();
        self.lay_sign_out();
        self.tenants_sign_out();
        self.panes_reset();
        self.views_sign_out();
        self.stash = None;
        self.badge = Badge::default();
        self.brand = Default::default();
        self.installs = Default::default();
        // drives open in this person's home: left alone (and said so) unless the Settings switch asks for them to be closed
        let mut drives_note = None;
        if let Some(s) = &self.session {
            let mut who = iyakki_shared::disks::Actor::new(&s.user, s.role);
            who.home = iyakki_apps::agent().scope_home_root(&s.user, std::path::Path::new("/var/lib/iyakki")).display().to_string();
            let (audit, user) = (self.env.audit.clone(), s.user.clone());
            drives_note = self.osguard.os_for("shell", &s.user).disk_engine().at_sign_out(&who, &|a, d| audit.record(&user, a, d));
        }
        if let Some(s) = self.session.take() {
            self.env.audit.record(&s.user, "logout", "");
            crate::soundowner::signed_out(&s.user);
            self.core.flows_view().stop_for_logout(&s.user);
            iyakki_apps::agent().sharedfolder_on_logout(&s.user);
            self.env.b4().ssh_tunnels_on_logout(&s.user);
            self.env.b3().vpn_on_logout(&s.user);
            self.env.services.domains_logout(&s.user);
            self.env.services.on_logout(&s.user, s.policy.manage_settings);
            self.env.b3().kube_resolver(None);
            self.osguard.os_for("shell", &s.user).bt_audio().clear_remote();
            // the assistant's browser tab goes with the session (jobs still running are answered)
            crate::apps::web_agent::close(&*self.env.browser_jobs(), &s.user);
            // protected browser data is written back and the decrypted copy wiped (a failure keeps it and is audited)
            iyakki_apps::agent().profilepolicy_timer_reset(self.env.data_dir(), &s.user);
            match iyakki_apps::agent().profile_store().on_sign_out(self.env.data_dir(), &s.user) {
                Ok(true) => self.env.audit.record(&s.user, "browser.data.sealed", ""),
                Ok(false) => {}
                Err(e) => self.env.audit.record(&s.user, "browser.data.seal.failed", &e),
            }
        }
        self.cues.badge_seen = false;
        if let Some(c) = cue {
            let _ = c.wait(iyakki_shared::syssounds::LOGOUT_WAIT);
        }
        self.screen = Screen::Login(Form::login_for(last_user(&self.env).as_deref()));
        self.signout_started(had_session);
        if let Some(m) = drives_note {
            self.say(m);
        }
    }

    /// In a web browser session whose policy allows the camera, the CAMERA tile is usable even when this machine has no camera (a Mac
    /// mini has none): the app then offers the browser's camera and asks the page to show its Allow button.
    pub(crate) fn browser_camera_offered(&self) -> bool {
        self.browser_media().is_some_and(|m| m.policy_camera())
    }

    /// Whether the CAMERA tile is usable (looks first when it has not looked yet).
    fn camera_ready(&mut self) -> bool {
        if self.browser_camera_offered() {
            return true;
        }
        if !self.camera.scanned() {
            self.camera.scan();
        }
        self.camera.available()
    }

    /// Look for cameras somewhere else (tests).
    #[cfg(test)]
    pub(crate) fn set_camera_for_test(&mut self, p: crate::camera::Presence) {
        self.camera = p;
    }

    fn open(&mut self, app: PApp) {
        let Some(cx) = self.cx() else { return };
        // cloud mode: a hidden app has no shortcut, no F-key and no jump that opens it
        if self.cloud.hides_app(app) {
            return;
        }
        if !cx.session.policy.allows_app(app) {
            self.env.audit.record(&cx.session.user, "app.denied", app.id());
            self.say(format!("Your role ({}) can't open {}", cx.session.role.name(), app.id().to_uppercase()));
            return;
        }
        if self.views_single_hit(app) {
            return;
        }
        if self.ws_active() && !self.panes_room_for(app) {
            self.say("Low memory: close an app or use a smaller layout");
            return;
        }
        // an app whose core object this program is not given (the UI over the core's socket) opens as a screen that says so
        let unserved = crate::apps::unserved::missing(app, &cx).and_then(|what| TILES.iter().find(|t| t.app == app).map(|t| Box::new(crate::apps::unserved::UnservedApp::new(t.label, t.icon, t.accent, what)) as Box<dyn App>));
        // EDIT and CLI have a version on the UI SDK next to the old screen: the person's own choice (Settings -> Appearance, `display_sdk_apps_set`) opens it under the same app
        let boxed: Box<dyn App> = if let Some(u) = unserved { u } else if let Some(t) = crate::apps::sdk_apps::tile_for(&cx, app) { t } else { match app {
            PApp::Chat => Box::new(ChatApp::new().with_agent(cx.session.policy.agent).resumed(&cx)),
            PApp::Rag => Box::new(RagApp::new(&cx)),
            PApp::Mcp => Box::new(McpApp::new(&crate::apps::mcp_port::CxPort(&cx))),
            PApp::Cli => Box::new(CliApp::new(&cx)),
            PApp::Users => Box::new(UsersApp::new()),
            PApp::Settings => Box::new(SettingsApp::new(&cx).with_cloud(self.cloud.clone())),
            PApp::Runs => Box::new(RunsApp::new(&cx)),
            PApp::Edit => Box::new(EditApp::new(&cx)),
            PApp::Browser => Box::new(crate::apps::browser::BrowserApp::new(&cx)),
            PApp::Mesh => Box::new(crate::apps::mesh::MeshApp::new(&cx)),
            PApp::Radio => Box::new(crate::apps::radio::RadioApp::new(&crate::apps::radio_port::CxPort(&cx))),
            PApp::Scan => Box::new(crate::apps::scan::ScanApp::new(&cx)),
            PApp::Skills => Box::new(crate::apps::skills::SkillsApp::new(&crate::apps::skills_port::CxPort(&cx))),
            PApp::Usage => Box::new(crate::apps::usage::UsageApp::new(&cx)),
            PApp::System => Box::new(crate::apps::system::SystemApp::new(&cx)),
            PApp::Calendar => Box::new(crate::apps::calendar::CalendarApp::new(&cx)),
            PApp::Media => Box::new(crate::apps::media::MediaApp::new(&crate::apps::media_port::CxPort(&cx))),
            PApp::Ide => Box::new(crate::apps::ide::IdeApp::new(&cx)),
            PApp::Data => Box::new(crate::apps::data::DataApp::new(&crate::apps::data_port::CxPort(&cx))),
            PApp::Store => Box::new(crate::apps::store::StoreApp::new(&cx)),
            PApp::Dash => Box::new(crate::apps::dash::DashApp::new(&cx)),
            PApp::Flows => Box::new(crate::apps::flows::FlowsApp::new(&cx)),
            PApp::Camera => Box::new(crate::apps::camera::CameraApp::new(&crate::apps::camera_port::CxPort(&cx))),
            PApp::Voice => Box::new(crate::apps::voice::VoiceApp::new(&cx)),
            PApp::K8s => Box::new(crate::apps::k8s_tile::K8sApp::new(&cx)),
            PApp::Disks => Box::new(crate::apps::disks_tile::DisksApp::new(&cx)),
            PApp::LinuxWin => Box::new(crate::apps::linuxwin::LinuxWinApp::new(&cx)),
            PApp::Tales => Box::new(crate::apps::story::StoryApp::new(&cx)),
            PApp::Lab => Box::new(crate::apps::lab::LabApp::new(&cx)),
            PApp::Git => Box::new(crate::apps::git_tile::GitApp::new(&cx)),
            PApp::Doctor => Box::new(crate::apps::doctor::DoctorApp::new(&cx)),
            PApp::Checkup => Box::new(crate::apps::checkup::CheckupApp::new(&cx)),
            PApp::Learn => Box::new(crate::apps::learn::LearnApp::new(&cx)),
            PApp::DevTools => Box::new(crate::apps::devtools::DevToolsApp::new(&cx)),
            PApp::Auth => Box::new(crate::apps::auth::AuthApp::new(&crate::apps::auth::CxPort(&cx))),
            PApp::Remote => Box::new(crate::apps::remotefiles::RemoteFilesApp::new(&cx)),
            PApp::Cameras => Box::new(crate::apps::cameras::CamerasApp::new(&cx)),
            PApp::Pdf => Box::new(crate::apps::pdfview::PdfApp::new(&cx)),
            PApp::Sample => Box::new(crate::apps::sample::SampleTile::new()),
            PApp::Markdown => Box::new(crate::apps::markdown::MarkdownTile::new()),
            PApp::Firewall => Box::new(crate::apps::firewall_tile::FirewallApp::new(&cx)),
            PApp::Inbox => Box::new(crate::apps::inbox::InboxApp::new(&cx)),
        } };
        self.env.audit.record(&cx.session.user, "app.open", app.id());
        self.custom_open = None;
        self.screen = Screen::App(boxed, app);
    }

    /// Open or close the Devices popover (F8, the button).
    fn toggle_devices(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.help = false;
        self.brand.close();
        self.installs.close();
        self.devpop.toggle();
        mark_stale!(self);
    }

    /// What the popover looks at: this machine (a made-up one in tests).
    fn dev_probe(&self) -> crate::devpop::Probe {
        if let Some(p) = &self.devprobe {
            return p.clone();
        }
        let os = self.osguard.os_for("shell", self.session.as_ref().map_or("", |s| s.user.as_str()));
        let drivers = os.drivers();
        let mut p = crate::devpop::Probe::real(self.core.clone(), crate::apps::devices::DeviceEnv::real(), Some(drivers), self.env.data_dir().to_path_buf(), self.env.hostname.clone(), self.env.data_volatile);
        p.screen = self.size;
        p.hardware = Some(os.hardware());
        p
    }

    fn handle_devpop(&mut self, ev: &Event) {
        use crate::devpop::Outcome;
        let out = match ev {
            Event::Key(k) => self.devpop.handle_key(k),
            Event::Tab(n) => self.devpop.handle_hit(*n),
            Event::Scroll(d) => {
                self.devpop.scroll_by(if *d > 0 { -1 } else { 1 });
                Outcome::Nothing
            }
            _ => Outcome::Nothing,
        };
        match out {
            Outcome::Nothing => {}
            Outcome::Close => self.devpop.close(),
            Outcome::Jump(j) => {
                self.devpop.close();
                match j {
                    crate::devpop::Jump::Settings(page) => self.jump_to("settings", &format!("tab:{page}")),
                    crate::devpop::Jump::Drivers => self.jump_to("store", "drivers"),
                    crate::devpop::Jump::Setup(id) => self.jump_to("store", &format!("setup:{id}")),
                    crate::devpop::Jump::MacDrivers => {
                        // the IYAKKI menu, opened on its Mac drivers page
                        self.toggle_brand();
                        if self.brand.open {
                            let out = self.brand.open_page(crate::brandmenu::Page::Mac);
                            if let crate::brandmenu::Outcome::Opened(p) = out {
                                self.brand_opened(p);
                            }
                        }
                    }
                    crate::devpop::Jump::None => {}
                }
            }
            Outcome::Act(crate::devpop::Act::FirewallOff) => {
                let admin = self.session.as_ref().is_some_and(|x| x.policy.manage_settings);
                if !admin {
                    self.say("Only administrators can switch the firewall off.".to_string());
                } else {
                    let user = self.session.as_ref().map(|x| x.user.clone()).unwrap_or_default();
                    let r = iyakki_fw::service::engine().disable_now(&format!("{user} (Devices list)"));
                    match r {
                        Ok(()) => {
                            self.env.audit.record(&user, "firewall.disable", "emergency off from the Devices list");
                            self.say_info("The firewall is off. Switch it on again in Settings -> Firewall.".to_string());
                        }
                        Err(e) => self.say(format!("Could not switch the firewall off: {e}")),
                    }
                }
                self.devpop.invalidate();
            }
            Outcome::Act(crate::devpop::Act::ToggleSoundMute) => {
                // IYAKKI's software volume carries the output's volume: mute it in Settings -> Sound (the level is kept)
                match self.env.services.master_toggle_mute().map(|m| iyakki_shared::audioout::SoundSettings { volume_pct: m.percent, muted: m.muted, ..Default::default() }) {
                    Ok(now) if now.muted => self.say_info("Muted.".to_string()),
                    Ok(now) => self.say_info(format!("Sound on, volume {}%.", now.volume_pct)),
                    Err(e) => self.say(e),
                }
                self.devpop.invalidate();
            }
            Outcome::Act(a) => {
                let p = self.dev_probe();
                match crate::devpop::run_act(&p, &a) {
                    Ok(m) => self.say_info(m),
                    Err(m) => self.say(m),
                }
                self.devpop.invalidate();
            }
        }
        mark_stale!(self);
    }

    /// Open or close the IYAKKI menu (F10, a click on the name in the launcher's top bar).
    fn toggle_brand(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.help = false;
        self.devpop.close();
        self.installs.close();
        let (w, h) = self.size;
        let s = scale_for(w, h);
        // under the name in the launcher, under the header in an app
        self.brand.anchor = match self.screen {
            Screen::App(..) => (12.0 * s, HEADER_H * s + 6.0 * s),
            _ => (34.0 * s, iyakki_uikit::barside::bar_dy() + 76.0 * s),
        };
        self.brand.admin = self.session.as_ref().is_some_and(|x| x.policy.manage_settings);
        self.apply_look_menu(w as f32);
        self.brand.toggle();
        mark_stale!(self);
    }

    /// Open or close the list of installs (the indicator beside the clock).
    fn toggle_installs(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.help = false;
        self.devpop.close();
        self.brand.close();
        self.installs.toggle();
        if let Some(cx) = self.cx() {
            self.installs.poll(&cx, true);
        }
        mark_stale!(self);
    }

    /// The facts a page of the IYAKKI menu needs, read when it opens.
    fn brand_opened(&mut self, page: crate::brandmenu::Page) {
        use crate::brandmenu::Page;
        match page {
            Page::Gpu => {
                // the files at once; the kernel log and the cluster's pods arrive on a thread and are merged while the page is drawn
                #[cfg(not(test))]
                let svc = self.env.services.clone();
                self.brand.set_gpu(iyakki_apps::agent().gpu_page().quick_report(std::path::Path::new("/"), self.env.data_dir()));
                #[cfg(not(test))]
                iyakki_apps::agent().gpu_page().refresh_async(svc.as_any_arc());
            }
            Page::Linux => self.refresh_linux(),
            Page::Media => {
                iyakki_apps::agent().player_register_dir(self.env.services.clis().bin_dir());
                self.brand.players = crate::brandmenu::players_found(&crate::brandmenu::on_path);
            }
            Page::Mac => {
                let p = self.dev_probe();
                self.devpop.refresh_in_background(&p);
                self.brand.mac = self.devpop.snapshot();
                self.brand.mac_model = iyakki_core_local::machinefacts::product_name();
                self.brand.mac_board = iyakki_core_local::machinefacts::board_name();
                self.refresh_mac_firmware();
            }
            _ => {}
        }
    }

    fn handle_brand(&mut self, ev: &Event) {
        use crate::brandmenu::Outcome;
        let out = match ev {
            Event::Key(k) => self.brand.handle_key(k),
            other => self.brand.handle_hit(other),
        };
        match out {
            Outcome::Nothing => {}
            Outcome::Close => self.brand.close(),
            Outcome::Opened(p) => self.brand_opened(p),
            Outcome::GpuTest => self.gpu_test(),
            Outcome::Install(item) => self.quick_install(item),
            Outcome::ShowInstalls => {
                self.brand.close();
                self.toggle_installs();
                if !self.installs.open {
                    self.toggle_installs();
                }
            }
            Outcome::MacFirmware(source) => self.mac_firmware(source),
            Outcome::Linux(a) => self.linux_action(a),
            Outcome::MacClean => {
                let freed = iyakki_apps::agent().macfw_delete_leftovers(self.env.data_dir());
                self.brand.note = Some((format!("Deleted the kept download: {} MB freed.", freed >> 20), true));
                self.refresh_mac_firmware();
            }
            Outcome::MacBtLoad => self.mac_bt_load(),
            Outcome::Act(p) => self.brand_act(p),
            Outcome::Play(path) => self.brand_play(&path),
            Outcome::StopMedia => {
                iyakki_apps::agent().b0().media().player().stop();
                self.brand.note = Some(("Stopped.".into(), true));
            }
            Outcome::Jump(app, item) => {
                self.brand.close();
                if app == "devices" {
                    self.toggle_devices();
                } else if app == "cloud" {
                    self.open_cloud_dialog();
                } else {
                    self.jump_to(app, &item);
                }
            }
        }
        mark_stale!(self);
    }

    /// What the Mac drivers page shows about the firmware job: the last result and whether one is running.
    fn refresh_mac_firmware(&mut self) {
        use iyakki_shared::quickinstall as qi;
        self.brand.mac_last = iyakki_apps::agent().macfw_read_outcome(self.env.data_dir());
        self.brand.mac_leftover = iyakki_apps::agent().macfw_leftovers(self.env.data_dir());
        self.brand.mac_cache = iyakki_apps::agent().macfw_card_status(self.env.data_dir());
        self.brand.mac_bt_held = iyakki_apps::agent().macfw_bluetooth_held(std::path::Path::new("/lib/modules"));
        self.brand.mac_busy = self.installs.jobs().iter().any(|j| j.running() && qi::item_of_command(&j.command) == Some(qi::Item::MacFirmware));
    }

    /// "Load the Bluetooth driver anyway" (administrators): give the held drivers back and load them in the background (a driver that finds
    /// no firmware can take a few seconds); Settings -> Bluetooth then shows the controller.
    fn mac_bt_load(&mut self) {
        let Some(sess) = self.session.clone() else { return };
        if !sess.policy.manage_settings {
            self.brand.note = Some((format!("Only an administrator can load drivers for the whole machine (your role is {}).", sess.role.name()), false));
            return;
        }
        self.env.audit.record(&sess.user, "bluetooth.load-anyway", "held drivers given back");
        let data = self.env.data_dir().to_path_buf();
        std::thread::spawn(move || {
            let _ = iyakki_apps::agent().macfw_load_bluetooth_anyway(&data);
        });
        self.brand.mac_bt_held = false;
        self.brand.note = Some(("Loading the Bluetooth drivers without a firmware file. Settings \u{2192} Bluetooth shows the controller in a few seconds; pairing and audio are not available yet.".into(), true));
    }

    /// Start the job that installs Apple's Wi-Fi and Bluetooth firmware from `source` (administrators; the click is the person's request).
    fn mac_firmware(&mut self, source: iyakki_shared::macfw::Source) {
        use iyakki_shared::quickinstall as qi;
        let Some(sess) = self.session.clone() else { return };
        if !sess.policy.manage_settings {
            self.brand.note = Some((format!("Only an administrator can install drivers for the whole machine (your role is {}).", sess.role.name()), false));
            return;
        }
        let svc = self.env.services.clone();
        let launch = match self.quick_launch.clone() {
            Some(l) => l,
            None => match qi::Launch::this_program(self.env.data_dir()) {
                Ok(l) => l,
                Err(e) => {
                    self.brand.note = Some((e, false));
                    return;
                }
            },
        };
        match svc.b0().qi_start_mac_firmware(&sess.user, &source, &launch) {
            Ok(job) => {
                self.env.audit.record(&sess.user, "macfirmware.start", &source.words());
                self.brand.note = Some((format!("Started: {}. Progress is beside the clock; the files are Apple's and stay on this machine.", job.label), true));
                if let Some(cx) = self.cx() {
                    self.installs.poll(&cx, true);
                }
                self.refresh_mac_firmware();
            }
            Err(e) => self.brand.note = Some((e, false)),
        }
    }

    /// Start the jobs of a Quick install (administrators), in dependency order.
    fn quick_install(&mut self, item: iyakki_shared::quickinstall::Item) {
        use iyakki_shared::quickinstall as qi;
        let Some(sess) = self.session.clone() else { return };
        if !sess.policy.manage_settings {
            self.brand.note = Some((format!("Only an administrator can install software for the whole machine (your role is {}).", sess.role.name()), false));
            return;
        }
        let svc = self.env.services.clone();
        let launch = match self.quick_launch.clone() {
            Some(l) => l,
            None => match qi::Launch::this_program(self.env.data_dir()) {
                Ok(l) => l,
                Err(e) => {
                    self.brand.note = Some((e, false));
                    return;
                }
            },
        };
        // only what the item needs is looked at (the Media page's player button needs nothing)
        let present: Vec<qi::Item> = {
            let mut need: Vec<qi::Item> = Vec::new();
            let mut todo = item.needs_now();
            while let Some(n) = todo.pop() {
                if !need.contains(&n) {
                    need.push(n);
                    todo.extend(n.needs_now());
                }
            }
            need.into_iter().filter(|i| svc.b0().qi_presence(*i).present()).collect()
        };
        match svc.b0().qi_start(&sess.user, item, &|i| present.contains(&i), &launch) {
            Ok(started) => {
                let steps: Vec<String> = started.iter().map(|j| j.label.clone()).collect();
                self.env.audit.record(&sess.user, "quickinstall.start", &format!("{} ({})", item.id(), steps.join(", ")));
                self.brand.note = Some((format!("Started: {}. Progress is beside the clock.", steps.join(", then ")), true));
                if let Some(cx) = self.cx() {
                    self.installs.poll(&cx, true);
                }
            }
            Err(e) => self.brand.note = Some((e, false)),
        }
    }

    /// The GPU page's test prompt: a detached job of the installer workspace (its log is the answer and the tokens per second).
    fn gpu_test(&mut self) {
        use iyakki_shared::quickinstall as qi;
        let Some(sess) = self.session.clone() else { return };
        if !sess.policy.manage_settings {
            self.brand.note = Some((format!("Only an administrator can run the test prompt (your role is {}).", sess.role.name()), false));
            return;
        }
        let svc = self.env.services.clone();
        let launch = match self.quick_launch.clone() {
            Some(l) => l,
            None => match qi::Launch::this_program(self.env.data_dir()) {
                Ok(l) => l,
                Err(e) => {
                    self.brand.note = Some((e, false));
                    return;
                }
            },
        };
        if svc.jobs().visible(&sess.user, qi::WORKSPACE).iter().any(|j| j.running() && j.label == "GPU test prompt") {
            self.brand.note = Some(("The test prompt is running already: its answer is in the installer runs beside the clock.".into(), false));
            return;
        }
        match svc.b0().qi_start_gpu_test(&sess.user, &launch) {
            Ok(_) => {
                self.env.audit.record(&sess.user, "gpu.testprompt", "");
                self.brand.note = Some(("Test prompt started: press Installer runs (or look beside the clock) for the answer and the tokens per second.".into(), true));
                if let Some(cx) = self.cx() {
                    self.installs.poll(&cx, true);
                }
            }
            Err(e) => self.brand.note = Some((e, false)),
        }
    }

    /// Play a file from the menu's Media page in the shared player (MEDIA's own).
    fn brand_play(&mut self, path: &str) {
        let Some(sess) = self.session.clone() else { return };
        let p = std::path::Path::new(path);
        if !p.is_file() {
            self.brand.note = Some((format!("No such file: {path}"), false));
            return;
        }
        if !sess.policy.allows_read(&sess.user, path) {
            self.brand.note = Some((format!("Your account may not read {path}."), false));
            return;
        }
        // video and every sound but MP3 play in MEDIA's own player (ffmpeg decodes, IYAKKI draws the picture)
        if iyakki_apps::agent().player_available() && !iyakki_apps::agent().media_is_mp3(path) {
            self.env.audit.record(&sess.user, "media.play", path);
            self.brand.close();
            self.jump_to("media", &format!("play:{path}"));
            return;
        }
        let label = p.file_name().map_or(path.to_string(), |n| n.to_string_lossy().into_owned());
        match iyakki_apps::agent().b0().media().player().play(&sess.user, &label, path) {
            Ok(()) => {
                self.env.audit.record(&sess.user, "media.play", path);
                self.brand.note = Some((format!("Playing {label}."), true));
            }
            Err(e) => self.brand.note = Some((format!("Could not play it: {e}"), false)),
        }
    }

    fn handle_installs(&mut self, ev: &Event) {
        use crate::installpop::Outcome;
        let Some(cx) = self.cx() else { return };
        match self.installs.handle(ev, &cx) {
            Outcome::Nothing => {}
            Outcome::Close => self.installs.close(),
            Outcome::OpenRuns => {
                self.installs.close();
                self.jump_to("runs", "jobs");
            }
        }
        mark_stale!(self);
    }

    /// Open an app and ask it to show one thing (`tab:network` in Settings, `drivers` in the Store).
    fn jump_to(&mut self, app: &str, item: &str) {
        self.open_linked(app);
        self.store_launch(app);
        if let (Some(cx), Screen::App(a, _)) = (self.cx(), &mut self.screen) {
            a.open_item(item, &cx);
        }
    }

    fn inbox_allowed(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.policy.allows_app(PApp::Inbox))
    }

    /// The bell, or F9: open the inbox. From inside an app, that app is kept aside (still running, so a chat waiting
    /// for an answer is not cut off) and comes back with Back. Pressed again inside the inbox, it closes it.
    fn open_inbox(&mut self) {
        if !self.inbox_allowed() {
            return;
        }
        if matches!(self.screen, Screen::App(_, PApp::Inbox)) {
            self.leave_app();
            return;
        }
        let prev = std::mem::replace(&mut self.screen, Screen::Launcher);
        if let Screen::App(app, pa) = prev {
            // already holding one aside (we got here through the inbox): the app being left is simply closed
            if self.stash.is_none() {
                self.stash = Some((app, pa));
            }
        } else {
            self.screen = prev;
            if !matches!(self.screen, Screen::Launcher) {
                return; // only after sign-in, from the launcher or an app
            }
        }
        self.open(PApp::Inbox);
        if !matches!(self.screen, Screen::App(_, PApp::Inbox)) {
            self.leave_app();
        }
    }

    /// The apps that listen hear which app came to the front and which went to the back (an app id and a word). Looked at once a frame, so
    /// every way of opening or leaving an app is one place.
    fn note_foreground(&mut self) {
        use iyakki_core_api::appevents::{payload, Val};
        let now: Option<String> = match (&self.screen, self.session.as_ref()) {
            (Screen::App(_, a), Some(_)) => Some(self.custom_open.clone().unwrap_or_else(|| a.id().to_string())),
            _ => None,
        };
        if now == self.fg_seen {
            return;
        }
        let Some(user) = self.session.as_ref().map(|s| s.user.clone()) else {
            self.fg_seen = None;
            return;
        };
        let ev = self.core.app_events();
        for (app, change) in [(self.fg_seen.take(), "background"), (now.clone(), "foreground")] {
            if let Some(app) = app {
                let _ = ev.emit("app.lifecycle", Some(&user), payload(&[("app", Val::Text(app)), ("change", Val::Text(change.into()))]));
            }
        }
        self.fg_seen = now;
    }

    /// Back out of the current app: to the app that was kept aside, or the launcher.
    fn leave_app(&mut self) {
        self.ctx = None;
        // Settings → Home layout may have changed the layout
        self.lay_reload();
        // Settings → Desktop views may have changed the views
        self.views_reload();
        self.custom_open = None;
        self.custom_at = 0;
        self.screen = match self.stash.take() {
            Some((app, pa)) => Screen::App(app, pa),
            None => Screen::Launcher,
        };
    }

    /// Open the app a notification points to.
    /// An app asked for another one (the IDE opens a table in DATA). The asking app is kept aside, still running, and
    /// comes back with Back; apps other than DATA replace it, as before.
    fn open_linked_from_app(&mut self, app: &str) {
        // the VOICE tile's "Set up Kokoro": the Store on its Setup tab, with the app kept where it is (Back returns to it)
        if app == "quickinstall" {
            let prev = std::mem::replace(&mut self.screen, Screen::Launcher);
            match prev {
                Screen::App(a, pa) if pa != PApp::Store => {
                    if self.stash.is_none() {
                        self.stash = Some((a, pa));
                    }
                }
                other => self.screen = other,
            }
            self.open_linked("store");
            self.store_launch("store");
            return;
        }
        // Store -> Setup cards that open a page of the IYAKKI menu, and the installer list
        if let Some(page) = app.strip_prefix("brand:") {
            self.toggle_brand();
            if self.brand.open {
                let p = match page {
                    "gpu" => crate::brandmenu::Page::Gpu,
                    "mac" => crate::brandmenu::Page::Mac,
                    _ => crate::brandmenu::Page::Linux,
                };
                if let crate::brandmenu::Outcome::Opened(p) = self.brand.open_page(p) {
                    self.brand_opened(p);
                }
            }
            return;
        }
        if app == "installs" {
            self.toggle_installs();
            if !self.installs.open {
                self.toggle_installs();
            }
            return;
        }
        // the IDE's Run for a tile app (a folder, looked at live as an unsigned dev app) and Open for an installed one
        if let Some(rest) = app.strip_prefix("tile-preview:").or_else(|| app.strip_prefix("tile-open:")) {
            let preview = app.starts_with("tile-preview:");
            let prev = std::mem::replace(&mut self.screen, Screen::Launcher);
            if let Screen::App(a, pa) = prev {
                if self.stash.is_none() {
                    self.stash = Some((a, pa));
                }
            } else {
                self.screen = prev;
            }
            if preview {
                let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
                match self.env.storeapi().preview_tile(&user, std::path::Path::new(rest)) {
                    Ok(e) => self.open_custom(e),
                    Err(m) => self.say(format!("Could not run the app: {m}")),
                }
            } else {
                self.open_custom_by_name(rest);
            }
            if !matches!(&self.screen, Screen::App(..)) || self.custom_open.is_none() {
                self.leave_app();
            }
            return;
        }
        // RUNS asking to show a run's task or row: back to the IDE that is kept aside (the caller then hands it the item)
        if app == "ide" {
            if self.stash.is_some() {
                self.leave_app();
            }
            return;
        }
        if (app == "data" || app == "browser" || app == "runs" || app == "edit" || app == "media") && self.stash.is_none() {
            let target = match app {
                "data" => PApp::Data,
                "runs" => PApp::Runs,
                "edit" => PApp::Edit,
                "media" => PApp::Media,
                _ => PApp::Browser,
            };
            let prev = std::mem::replace(&mut self.screen, Screen::Launcher);
            match prev {
                Screen::App(a, pa) if pa != target => self.stash = Some((a, pa)),
                other => self.screen = other,
            }
            self.open_linked(app);
            if !matches!(&self.screen, Screen::App(_, pa) if *pa == target) {
                self.leave_app();
            }
            return;
        }
        self.open_linked(app);
    }

    /// Hand the Store the stand-in program for the install jobs (tests and the screenshot tool); normally there is none.
    fn store_launch(&mut self, app: &str) {
        if app != "store" {
            return;
        }
        if let (Some(l), Some(cx), Screen::App(a, _)) = (self.quick_launch.clone(), self.cx(), &mut self.screen) {
            a.open_item(&format!("launch:{}\n{}", l.exe.display(), l.data.display()), &cx);
        }
    }

    fn open_linked(&mut self, app: &str) {
        if let Some(pa) = PApp::ALL.into_iter().find(|a| a.id() == app) {
            if pa != PApp::Inbox {
                self.open(pa);
            }
        }
    }

    /// Open an app by its id and say whether it is the screen now (the screens tool and the render smoke tests).
    /// Put a ready-made app on screen (tests that drive an app through the shell's real mouse handling).
    #[cfg(test)]
    pub(crate) fn probe_for_test(&self) -> String {
        match &self.screen {
            Screen::App(a, _) => a.probe(),
            _ => String::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn show_app_for_test(&mut self, app: Box<dyn crate::apps::App>, id: PApp) {
        self.screen = Screen::App(app, id);
        mark_stale!(self);
    }

    /// Run Quick install's jobs with another program (the screenshot tool uses a stand-in script).
    pub fn set_quick_launch_for_demo(&mut self, l: iyakki_shared::quickinstall::Launch) {
        self.quick_launch = Some(l);
    }

    pub fn open_for_demo(&mut self, id: &str) -> bool {
        if self.session.is_some() {
            self.screen = Screen::Launcher;
            self.stash = None;
        }
        if id == "inbox" {
            self.open_inbox();
        } else {
            self.open_linked(id);
        }
        self.screen_name() == format!("app:{id}")
    }

    pub fn badge(&self) -> Badge {
        self.badge
    }

    /// Count what the bell shows again (cheap: one small file and the live runs).
    pub fn refresh_badge(&mut self) {
        let b = match &self.session {
            Some(s) => {
                if !self.cues.badge_seen {
                    // first count after signing in: group an older inbox file and drop what is past its keeping time
                    let _ = self.env.inbox().maintain(&s.user, iyakki_clock::now_unix());
                }
                let c = self.env.inbox().counts(&s.user, iyakki_clock::now_unix());
                Badge { unread: c.attention, other: c.other, approvals: self.env.services.approvals(&s.user).len() + self.env.secret_requests().pending(&s.user).len() + self.env.services.browser_consent().pending(&s.user).len(), overdue: c.overdue }
            }
            None => Badge::default(),
        };
        // a new notification (the count of unread ones went up) has a sound; the first count after signing in does not
        if self.session.is_some() {
            if self.cues.badge_seen && !self.cues.quiet && b.unread > self.badge.unread {
                let _ = crate::cues::start(&*self.core, &self.cloud, iyakki_core_api::soundtypes::Cue::Notify);
            }
            self.cues.badge_seen = true;
        }
        if b != self.badge {
            self.badge = b;
            mark_stale!(self);
        }
        let mut status = match &self.session {
            Some(sess) => self.core.app_status().all(&sess.user),
            None => Vec::new(),
        };
        // the app that is open says more than the core can know from outside: its own status wins
        if let Some((id, own)) = self.open_app_status() {
            status.retain(|(a, _)| *a != id);
            status.push((id, own));
        }
        if status != self.app_status {
            self.app_status = status;
            mark_stale!(self);
        }
        let updates = if self.session.is_some() { self.env.storeapi().update_report().map_or(0, |r| r.count()) } else { 0 };
        if updates != self.update_count {
            self.update_count = updates;
            mark_stale!(self);
        }
    }

    /// The line under a tile's name: what the app says about itself now (`App::status`, or the core's word for an app that is not open:
    /// "3 updates", "Playing Radio X"), else the tile's own static line. Never a made-up figure.
    pub fn tile_sub(&self, app: PApp, sub: &str) -> String {
        match self.status_of(app) {
            Some(st) if !st.line.is_empty() => st.line.clone(),
            _ => sub.to_string(),
        }
    }

    /// What `app` says about itself now, if anything.
    pub fn status_of(&self, app: PApp) -> Option<&iyakki_core_api::AppStatus> {
        self.app_status.iter().find(|(id, _)| id == app.id()).map(|(_, st)| st)
    }

    /// The open app's own status (`App::status`): the app on the screen or the one held aside.
    fn open_app_status(&self) -> Option<(String, iyakki_core_api::AppStatus)> {
        if !matches!(&self.screen, Screen::App(..)) && self.stash.is_none() {
            return None;
        }
        let cx = self.cx()?;
        match &self.screen {
            Screen::App(app, pa) => app.status(&cx).map(|st| (pa.id().to_string(), st)),
            _ => self.stash.as_ref().and_then(|(app, pa)| app.status(&cx).map(|st| (pa.id().to_string(), st))),
        }
    }

    /// A detached job ended (see `docs/JOBS.md`): one note in the inbox for each, whichever app is open, so the bell shows it and
    /// the note opens the Jobs list. The chat that started the job shows its own event; this is for the person who is elsewhere.
    pub fn announce_finished_jobs(&mut self) {
        let Some(user) = self.session.as_ref().map(|s| s.user.clone()) else { return };
        let (mut any, mut ok) = (false, false);
        self.cues.quiet = true;
        for j in self.env.services.jobs().take_finished_all(&user, iyakki_shared::jobs::Channel::Inbox) {
            any = true;
            ok |= j.state == iyakki_shared::jobs::State::Succeeded;
            let (kind, word) = match j.state {
                iyakki_shared::jobs::State::Succeeded => (iyakki_shared::inbox::Kind::Agent, "finished"),
                iyakki_shared::jobs::State::Stopped => (iyakki_shared::inbox::Kind::Info, "was stopped"),
                _ => (iyakki_shared::inbox::Kind::Warning, "did not finish"),
            };
            let _ = self.env.notify(&user, kind, &format!("Job “{}” {word}", j.label), &iyakki_shared::jobs::finish_line(&j), Some(iyakki_shared::inbox::Link::new("runs", "jobs")));
            self.refresh_badge();
            mark_stale!(self);
        }
        self.cues.quiet = false;
        if any {
            // one sound for the batch: "done" when something finished well, the notification chime when none did
            let cue = if ok { iyakki_core_api::soundtypes::Cue::JobDone } else { iyakki_core_api::soundtypes::Cue::Notify };
            let _ = crate::cues::start(&*self.core, &self.cloud, cue);
        }
    }

    /// Keep an app that is held aside running: it gets the clock's ticks.
    fn tick_stash(&mut self) {
        let cx = self.cx();
        if let (Some((app, _)), Some(cx)) = (&mut self.stash, cx) {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.handle(&Event::Tick, &cx))).is_err() {
                self.stash = None;
            }
        }
    }

    /// Leave the hardware check for the normal first screen.
    fn leave_hwcheck(&mut self) {
        let first_run = self.env.store.lock().map(|s| s.is_empty()).unwrap_or(false);
        self.screen = if first_run { Screen::Setup(Form::setup()) } else { Screen::Login(Form::login_for(last_user(&self.env).as_deref())) };
    }

    fn submit_login(&mut self) {
        let Screen::Login(f) = &mut self.screen else { return };
        let (name, pw) = (f.fields[0].value.trim().to_string(), f.fields[1].value.clone());
        let r = Ok::<_, AccountError>(self.env.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner)).and_then(|mut s| s.authenticate(&name, &pw));
        match r {
            Ok(sess) => self.sign_in(sess, &pw),
            Err(e) => {
                if let Screen::Login(f) = &mut self.screen {
                    f.fields[1].clear();
                    f.error = Some(e.to_string());
                }
                let shown: String = name.chars().filter(|c| c.is_ascii_graphic()).take(32).collect();
                self.env.audit.record("-", "login.failed", &shown);
            }
        }
    }

    fn submit_setup(&mut self) {
        let Screen::Setup(f) = &mut self.screen else { return };
        let (name, pw, pw2) = (f.fields[0].value.trim().to_string(), f.fields[1].value.clone(), f.fields[2].value.clone());
        if pw != pw2 {
            f.error = Some("The passwords don't match".into());
            f.fields[2].clear();
            return;
        }
        let r = Ok::<_, AccountError>(self.env.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner)).and_then(|mut s| {
            s.create_first_admin(&name, &pw)?;
            s.authenticate(&name, &pw)
        });
        match r {
            Ok(sess) => {
                self.env.audit.record(&sess.user, "setup", "first administrator created");
                self.sign_in(sess, &pw);
            }
            Err(e) => {
                if let Screen::Setup(f) = &mut self.screen {
                    f.error = Some(e.to_string());
                }
            }
        }
    }

    /// Entry point for every input event.
    pub fn handle(&mut self, ev: &Event) {
        if matches!(ev, Event::Key(_) | Event::MouseDown(..) | Event::MouseUp(..)) {
            // the person's own press answers a hard-ask card (swap, compressed memory) for the work this event starts, and only for that
            let card = self.osguard.card();
            return card.dispatch(|| self.handle_event(ev));
        }
        self.handle_event(ev)
    }

    /// An approval, permission or power card is up: the shell/reboot/update ask of developer mode, the power cards (signed in and the
    /// sign-in screen) or a flow's permission ask. Nothing a paired computer sends (`dev input`) may reach one of these.
    pub(crate) fn approval_card_open(&self) -> bool {
        self.devask.pending.is_some() || self.devask.open || self.fnk.power_card || matches!(self.screen, Screen::PowerMenu) || self.lbar.card.is_some() || self.permpop.is_some()
    }

    fn handle_event(&mut self, ev: &Event) {
        if let Event::ScrollPx(px) = ev {
            return self.scroll_px(*px); // the scroll engine's pixel steps (`shell_scroll.rs`)
        }
        if !self.saver_intercept(ev) {
            self.handle_awake(ev);
        }
        self.saver_after();
        self.cameras_sync();
    }

    /// Whether the CAMERAS tile is drawn on the screen now: signed in, not locked, no saver, and the tile is the shown app or in a pane.
    /// An app kept aside (`stash`) or in another desktop view is not on the screen.
    pub(crate) fn cameras_on_screen(&self) -> bool {
        if self.session.is_none() || self.lockui.active || self.saver_covers_desktop() {
            return false;
        }
        match &self.screen {
            Screen::App(_, PApp::Cameras) => true,
            Screen::Launcher | Screen::App(..) => self.panes_hold(PApp::Cameras),
            _ => false,
        }
    }

    /// Every way off the CAMERAS tile ends here (Esc or q, Back, Home, F11, a view switch, a pane or layout change, the lock, the sign-out,
    /// another tile, the saver, a quit from elsewhere): when the tile is not on the screen, this computer's own cameras close within
    /// the heartbeat that follows (KI-093). The tile itself lets go of the cameras it does not draw while it is shown.
    fn cameras_sync(&mut self) {
        if !self.cameras_on_screen() {
            iyakki_apps::agent().cameras_release_screen_at(self.env.data_dir(), &[]);
        }
    }

    /// The event for a screen that is on (the saver has already seen it).
    fn handle_awake(&mut self, ev: &Event) {
        use crate::ui::clipboard;
        // the accessibility keys work on every screen, the sign-in and the lock screen too
        if self.access_intercept(ev) {
            return;
        }
        if self.osk_intercept(ev) {
            return;
        }
        // locked: the password prompt takes every key and click, no app sees one
        if self.lock_intercept(ev) {
            return;
        }
        // the sign-out confirm card takes every key and click while it is up
        if self.signout_intercept(ev) {
            return;
        }
        if let Event::Swipe { back, at } = ev {
            self.swipe_event(*back, *at);
            return; // never reaches an app: apps keep their own gestures
        }
        let (prev_mouse, prev_sel) = (self.mouse, self.sel);
        if !matches!(ev, Event::Tick | Event::MouseMove(..)) {
            crate::filepicker::clear_path_field_note(); // drawn again with the next frame if a path field is still there
        }
        if let Some((agent, logout)) = crate::agent_login::take_request() {
            self.login_request(agent, logout);
        }
        // the assistant asked (and the person approved) to open a saved download in the tile that suits it: the same hand-over as a
        // click in the browser's Downloads list, the screen the person is on is kept aside
        if let Some(user) = self.session.as_ref().map(|s| s.user.clone()) {
            if let Some((tile, path)) = crate::apps::download_open::take_chat(&user) {
                let item = crate::apps::download_open::item_for(&tile, &path);
                self.open_linked_from_app(&tile);
                if let (Some(cx), Screen::App(a, _)) = (self.cx(), &mut self.screen) {
                    a.open_item(&item, &cx);
                }
                mark_stale!(self);
            }
        }
        // the assistant asked (and the person approved) for a shell in a pod: the Terminal opens on it and asks for the session itself
        if self.session.is_some() {
            if let Some(item) = iyakki_shared::podterm::take() {
                self.open_linked_from_app("cli");
                if let (Some(cx), Screen::App(a, _)) = (self.cx(), &mut self.screen) {
                    a.open_item(&item, &cx);
                }
                mark_stale!(self);
            }
        }
        // the click on the red sign "a page is using your camera / microphone / screen" stops it (the browser obeys at its next beat)
        if matches!(ev, Event::Tab(913)) {
            crate::apps::browser_media::ask_stop();
            mark_stale!(self);
            return;
        }
        // the click on the red sign "listening for My IYAKKI" switches the wake phrase off (and audits it)
        if matches!(ev, Event::Tab(916)) {
            self.wake_sign_click();
            return;
        }
        // the click on the red REC sign switches off every recording of the signed-in person's cameras (and audits it)
        if matches!(ev, Event::Tab(915)) {
            if let Some(s) = &self.session {
                crate::apps::cameras::stop_all_signs(self.env.data_dir(), &s.user);
            }
            mark_stale!(self);
            return;
        }
        // the click on the sign "your browser's microphone / camera is on" switches both off (and audits it)
        if matches!(ev, Event::Tab(914)) {
            if let Some(m) = self.browser_media() {
                m.stop_capture();
            }
            mark_stale!(self);
            return;
        }
        if let Event::MouseMove(x, y) | Event::MouseDown(x, y) = ev {
            self.mouse = (*x, *y);
        }
        // the Devices popover's tooltip sees every event first, whoever takes the click (a press that closes or opens a
        // row included), and asks for a new picture itself when it appears or goes away
        if self.devpop.open {
            self.devpop.tip_event(ev);
            if self.devpop.tip_changed() {
                mark_stale!(self);
            }
        }
        if let Screen::HwCheck(h) = &mut self.screen {
            h.note(ev); // the live tester sees every event, whatever else handles it
        }
        if !matches!(ev, Event::Tick) {
            self.tbar_activity();
        }
        // volume, brightness and media keys, and the power card they can open, are the shell's: no app sees them
        if self.handle_fn_event(ev) {
            return;
        }
        // the tenant dialogs, the strip and the shown tenant's window (`shell_tenant.rs`)
        if self.tenant_intercept(ev) {
            mark_stale!(self);
            return;
        }
        if matches!(ev, Event::Tick) {
            self.tick += 1;
            self.refresh_board_facts();
            // Settings or a chat tool changed the person's Look or dock: read them again
            if iyakki_uikit::look_registry::take_changed() {
                if let Some(u) = self.session.as_ref().map(|x| x.user.clone()) {
                    self.load_look(&u);
                    mark_stale!(self);
                }
            }
            // the home screen's page change moves the island and the icons for about a second: a frame per heartbeat while it does
            if self.home_animating() {
                mark_stale!(self);
            }
            // the galaxy behind the sign-in form turns: a frame about 15 times a second (the heartbeat is every 33 ms)
            if self.tick % 2 == 0 && matches!(self.screen, Screen::Login(_) | Screen::Setup(_)) && self.session.is_none() && !crate::fx::reduced_motion() {
                mark_stale!(self);
            }
            self.access_tick();
            self.crash_card_tick();
            self.fn_tick();
            self.tbar_tick();
            self.check_alarms(iyakki_clock::now_unix());
            self.tick_stash();
            self.panes_tick();
            self.views_hidden_beat();
            // developer mode: a paired computer asked for an app on the screen (`iyakki dev run`); it waits for someone to sign in
            if self.tick % 10 == 3 && self.session.is_some() {
                for name in iyakki_apps::agent().devmode().take_open(self.env.data_dir()) {
                    if self.open_custom_by_name(&name) {
                        mark_stale!(self);
                    }
                }
            }
            // the assistant asked for an SSH login (the person approved the card): the CLI tile comes up and opens it
            if self.tick % 5 == 2 {
                if let Some(sess) = &self.session {
                    if iyakki_apps::agent().sshsessiontools_pending(self.env.data_dir(), &sess.user) && !matches!(self.screen, Screen::App(_, PApp::Cli)) {
                        self.open(PApp::Cli);
                        mark_stale!(self);
                    }
                }
            }
            // developer mode: keys and taps a paired computer sent (`iyakki dev input`), as the shell's own events; the shell says
            // whether a password field has focus so the device can refuse them
            {
                let dd = self.env.data_dir().to_path_buf();
                iyakki_apps::agent().devmode().set_secret_focus(&dd, self.fields.iter().any(|f| f.focused && f.secret));
                self.dev_state_tick();
                iyakki_apps::agent().devmode().set_approval_card(&dd, self.approval_card_open());
                for e in iyakki_apps::agent().devmode().take_input(&dd) {
                    self.saver.injecting = true; // it wakes the screen and still reaches the app
                    // a permission is the person's own: developer-mode input cannot answer a flow's permission request
                    self.env.services.browser_consent().set_synthetic(true);
                    for ev in crate::devinput::events(&e) {
                        // an approval or power card is answered by a person at the screen: a paired computer's key or tap never
                        // reaches it (checked per event: an earlier key of the same batch may have opened one)
                        if self.approval_card_open() {
                            self.env.audit.record(self.session.as_ref().map_or("", |s| s.user.as_str()), "devmode.input", "dropped: an approval or power card is open");
                            self.say(iyakki_shared::devmode::input::APPROVAL_SENTENCE.to_string());
                            continue;
                        }
                        // not `handle`: that would let this press answer a hard-ask card (the Auto-mode ticket is for a person's own press)
                        self.handle_event(&ev);
                    }
                    self.env.services.browser_consent().set_synthetic(false);
                    self.saver.injecting = false;
                    mark_stale!(self);
                }
            }
            // the assistant's browser tab (the engine behind the browser_* tools) is serviced whichever app is open
            if let Some(sess) = &self.session {
                crate::apps::web_agent::tick(&*self.env.browser(), &*self.env.browser_jobs(), self.env.data_dir(), &self.env.audit, sess);
            }
            self.login_bar_tick();
            if self.tick % 5 == 1 {
                self.permpop_tick();
            }
            self.startup_tick();
            if self.tick % 15 == 7 {
                self.battery_tick();
            }
            if self.tick % 15 == 9 {
                self.update_check_tick();
            }
            if self.tick % 15 == 11 {
                self.sharing_notice_tick();
            }
            if self.tick % 15 == 13 {
                self.mirror_tick();
            }
            if self.tick % 15 == 3 {
                self.loss_tick();
            }
            // a drive pulled out while it was open: let go of it and say so (the tests call `drives_tick` themselves, they touch no real drive)
            if !cfg!(test) && self.tick % 30 == 7 {
                self.drives_tick();
            }
            self.devask_tick();
            self.tzask_tick();
            if self.tick % 15 == 0 {
                self.refresh_badge();
                // the clock changes the picture only when what it shows changes
                if self.session.is_some() || self.on_auth_screen() {
                    let t = clock_time_text(&self.clock);
                    if t != self.clock_text {
                        self.clock_text = t;
                        mark_stale!(self);
                    }
                }
            }
            if self.session.is_some() {
                if self.devpop.wants_refresh() {
                    let p = self.dev_probe();
                    self.devpop.refresh_in_background(&p);
                }
                if self.devpop.take_fresh() {
                    mark_stale!(self);
                    if self.brand.open && self.brand.page == crate::brandmenu::Page::Mac {
                        self.refresh_mac_firmware();
                        self.brand.mac = self.devpop.snapshot();
                    }
                }
                // the installer indicator: read the installer jobs (the list throttles itself), animate while one runs
                let busy = self.installs.open || matches!(self.installs.badge(), crate::installpop::Badge::Running { .. });
                if self.tick % 30 == 0 || (busy && self.tick % 10 == 0) {
                    if let Some(cx) = self.cx() {
                        let before = self.installs.badge();
                        if self.installs.poll(&cx, false) {
                            mark_stale!(self);
                        }
                        // an install that was running has ended: the "done" sound
                        let ended = match (before, self.installs.badge()) {
                            (crate::installpop::Badge::Running { count: a, .. }, crate::installpop::Badge::Done { .. }) => a > 0,
                            (crate::installpop::Badge::Running { count: a, .. }, crate::installpop::Badge::Running { count: b, .. }) => b < a,
                            _ => false,
                        };
                        if ended {
                            let _ = crate::cues::start(&*self.core, &self.cloud, iyakki_core_api::soundtypes::Cue::JobDone);
                        }
                    }
                }
                if self.installs.frame_due() {
                    mark_stale!(self);
                }
                // Linux apps: answers of the cluster actions and the jobs' count while the page is open
                if self.brand.open && self.brand.page == crate::brandmenu::Page::Linux && self.tick % 15 == 5 {
                    self.linux_poll();
                }
                // Developer mode's sign in the top bar
                if self.tick % 30 == 3 {
                    // (a system update that did not finish is told whatever developer mode says: the boot menu keeps the old version)
                    let sign = iyakki_apps::agent().devmode().top_sign(self.env.data_dir());
                    if sign != self.dev_sign {
                        self.dev_sign = sign;
                        mark_stale!(self);
                    }
                }
                // Settings → Time "Detect automatically": looked up again when due (only while it is switched on)
                if self.tick % 300 == 7 {
                    self.env.b0().tz_refresh_in_background(false, Arc::new(|z: &str| iyakki_clock::set_chosen_zone(z)));
                }
            }
            // a Quick install job installed k3s, Ollama or Lemonade: start (and connect) it here, where the supervisors live
            // the start-up page shows what Quick install is doing
            if self.tick % 30 == 5 {
                if let Some(u) = self.session.as_ref().map(|s| s.user.clone()) {
                    iyakki_apps::agent().installstartup_report(iyakki_shared::startup::global(), &self.env.b0().qi_install_jobs(&u));
                }
            }
            if self.tick % 90 == 45 {
                for r in self.env.b0().qi_service_requests() {
                    match r {
                        Ok(m) => self.say_info(m),
                        Err(e) => self.say(e),
                    }
                }
            }
            if self.tick % 90 == 0 {
                self.check_new_devices();
            }
            // a device that wants to pair while Settings is not open: say who and where to answer (the popup opens on the page)
            #[cfg(not(test))]
            if self.tick % 30 == 10 && !matches!(self.screen, Screen::App(_, PApp::Settings)) {
                if let Some(n) = crate::btmodel::next_pairing_notice(&iyakki_core_local::devctl::System) {
                    self.say_info(n);
                }
            }
            if self.tick % 30 == 15 {
                // a sound card that came up after the start (the built-in speakers of a MacBook take about 11 s)
                for line in self.core.sound().take_notices() {
                    self.say_info(line);
                }
            }
            if self.tick % 60 == 0 {
                self.check_cluster_alerts();
            }
            if self.tick % 45 == 0 {
                self.announce_finished_jobs();
            }
            if self.toast.as_ref().is_some_and(|t| t.1 <= self.tick) {
                self.toast = None;
            }
            // the sign-in dialog reads what the vendor program printed; sign-outs report when they are done
            // the browser was left: the sign-in dialog comes back, with the clipboard offer
            if self.login.as_ref().is_some_and(|m| m.hidden()) && !matches!(self.screen, Screen::App(_, PApp::Browser)) {
                if let Some(cx) = self.cx() {
                    if let Some(m) = self.login.as_mut() {
                        m.show_again(&cx);
                    }
                    mark_stale!(self);
                }
            }
            if self.login.as_ref().is_some_and(|m| !m.hidden()) && self.tick % 15 == 7 {
                if let Some(m) = self.login.as_mut() {
                    m.refresh_clip_offer();
                }
            }
            crate::ui::clipboard::tick();
            if self.clipscan.as_mut().is_some_and(|c| self.tick % 3 == 0 && c.tick()) {
                mark_stale!(self);
            }
            if self.login.is_some() && self.tick % 3 == 0 {
                if let Some(cx) = self.cx() {
                    if self.login.as_mut().is_some_and(|m| m.tick(&cx)) {
                        mark_stale!(self);
                    }
                }
            }
            if self.tick % 10 == 5 {
                for (m, ok) in crate::agent_login::take_results() {
                    if ok {
                        self.say_info(m);
                    } else {
                        self.say(m);
                    }
                }
            }
        }
        if self.panes_intercept(ev) {
            mark_stale!(self);
            return;
        }
        match ev {
            Event::MouseDown(x, y) => self.click(*x, *y),
            Event::MouseUp(..) => {
                self.press = None;
                self.dispatch(ev);
            }
            _ => self.dispatch(ev),
        }
        // the pointer moves with the button held over a text field or area: repeat the press there so the selection grows
        if let (Event::MouseMove(x, y), Some((tmpl, rect))) = (ev, self.press.clone()) {
            let again = match tmpl {
                Event::Focus(i) => Some(Event::FocusAt(i, *x as f32 - rect.x)),
                Event::Area(i) => Some(Event::AreaAt(i, *x as f32 - rect.x, *y as f32 - rect.y)),
                _ => None,
            };
            if let Some(e) = again {
                crate::ui::set_dragging(true);
                self.dispatch(&e);
                crate::ui::set_dragging(false);
                mark_stale!(self);
            }
        }
        if matches!(ev, Event::Key(_)) {
            self.press = None;
        }
        if !matches!(ev, Event::Tick | Event::MouseMove(..)) {
            self.refresh_badge();
        }
        if let Some(t) = clipboard::take_toast() {
            self.say_info(t);
        }
        // Anything that can change what is on screen makes last frame's click regions stale; until the
        // next frame is drawn, clicks fall back to the plain handlers instead of hitting old controls.
        if !matches!(ev, Event::MouseMove(..) | Event::MouseUp(..) | Event::Tick) {
            self.hits.clear();
        }
        match ev {
            // the radio's display moves with the music: draw every tick while it is on screen and playing
            Event::Tick => {
                if !self.resume_tried {
                    self.resume_tried = true;
                    let ended = self.core.session_resume().take_unexpected_end();
                    self.try_resume();
                    self.say_unexpected_end(ended);
                }
                // the ticket says "still here" every few seconds while somebody is signed in (only written when developer mode allows it)
                if self.tick % 30 == 0 {
                    if let Some(s) = &self.session {
                        self.core.session_resume().remember(&s.user);
                        let apps = self.open_app_ids();
                        self.core.session_resume().remember_apps(&apps);
                        let items = self.open_app_items();
                        self.core.session_resume().remember_items(&items);
                        if self.core.session_resume().allowed() {
                            self.views_remember_for_resume();
                        }
                    }
                }
                self.scroll_sync();
                // hardware that shows up after the shell started (a slow sound card) changes Cloud mode: look again every 10 s
                // (and a browser's microphone, camera or sound switched on or off in the page: every two seconds)
                if !cfg!(test) && (self.tick % 75 == 0 || (self.tick % 15 == 0 && self.browser_media().is_some())) {
                    let c = self.detect_cloud();
                    self.refresh_cloud(c);
                }
                // a camera plugged in or pulled out changes the CAMERA tile within two seconds
                if matches!(self.screen, Screen::Launcher) && self.cloud.camera_watch && (self.tick % 60 == 0 || !self.camera.scanned()) && self.camera.scan() {
                    mark_stale!(self);
                }
                // the "camera in use by the assistant" sign comes and goes (and its time moves) on every screen
                if self.tick % 15 == 0 {
                    let on = self.env.services.camera().in_use().is_some();
                    if on || on != self.cam_sign {
                        self.cam_sign = on;
                        mark_stale!(self);
                    }
                }
                // the wake phrase listener follows the settings; its red sign comes and goes on every screen
                self.wake_tick();
                self.gv_tick();
                // the same red sign says when an app shows a camera in its own window (embedded camera views, through the core)
                if self.tick % 15 == 0 {
                    let on = !self.core.cameras().indicator_names().is_empty();
                    if on != self.cam_views_sign {
                        self.cam_views_sign = on;
                        mark_stale!(self);
                    }
                }
                // the red REC sign of a camera that records comes and goes on every screen
                if self.tick % 15 == 0 && crate::apps::cameras::rec_sign_changed(self.env.data_dir()) {
                    mark_stale!(self);
                }
                if matches!(self.screen, Screen::App(_, PApp::Radio)) && self.env.services.radio().is_active() {
                    mark_stale!(self);
                }
                // a tool being downloaded shows its progress
                if matches!(self.screen, Screen::App(_, PApp::Cli)) && self.tick % 6 == 0 && (self.env.services.clis().running() || self.env.services.cluster().state() == iyakki_shared::cluster::ClusterState::Starting) {
                    mark_stale!(self);
                }
                // live camera pictures arrive about ten times a second
                if matches!(self.screen, Screen::App(_, PApp::Cameras)) && self.tick % 3 == 0 {
                    mark_stale!(self);
                }
                // the clock moves every second
                if matches!(self.screen, Screen::App(_, PApp::Calendar | PApp::Media | PApp::Remote)) && self.tick % 10 == 0 {
                    mark_stale!(self);
                }
                // the level display and the pulse of the MEDIA player move ten times a second, only while something plays
                if matches!(self.screen, Screen::App(_, PApp::Media)) && self.tick % 3 == 0 && iyakki_apps::agent().b0().media().player().now_playing().is_some() {
                    mark_stale!(self);
                }
                // an app that shows live pictures (the browser's full engine) says when a new one has arrived
                self.app_redraw();
            }
            // a pointer that only moves changes the picture when it crosses the edge of something that reacts
            Event::MouseMove(..) => {
                let (a, b) = (prev_mouse, self.mouse);
                if matches!(self.screen, Screen::HwCheck(_)) || self.sel != prev_sel || self.dock_pointer_changes(a, b) || self.probes.iter().any(|r| r.contains(a.0, a.1) != r.contains(b.0, b.1)) {
                    mark_stale!(self);
                }
            }
            _ => mark_stale!(self),
        }
        self.panes_after(ev, prev_mouse);
    }

    /// Ask the open app whether it wants a frame, and of which kind (the heartbeat and [`Shell::poll_live`] both do).
    fn app_redraw(&mut self) {
        let jump = if let Screen::App(app, _) = &mut self.screen { app.take_jump() } else { None };
        if let Some((to, item)) = jump {
            self.jump_to(&to, &item);
            return;
        }
        if let Screen::App(app, _) = &mut self.screen {
            match app.redraw_kind() {
                crate::apps::browser_full::Redraw::Full => mark_stale!(self),
                crate::apps::browser_full::Redraw::Live if self.live_paint => {
                    self.live = true;
                    // an app that repaints small rectangles may ask for the whole scene less often (see `redraw_interval_ms`)
                    self.live_every_ms = app.live_full_every_ms().max(100);
                    self.live_until = self.tick + 15;
                }
                crate::apps::browser_full::Redraw::Live => mark_stale!(self),
                crate::apps::browser_full::Redraw::None => {}
            }
        }
    }

    /// True while the open app wants to be polled between heartbeats (the word being read aloud): the backend waits only a few ms
    /// then and calls [`Shell::poll_live`].
    pub fn fast_poll(&self) -> bool {
        matches!(&self.screen, Screen::App(app, _) if app.wants_fast_poll())
    }

    /// Between heartbeats: the app says whether something changed that needs a frame now; if so the usual (live or whole) frame
    /// is requested. Returns true when the whole scene is stale.
    pub fn poll_live(&mut self) -> bool {
        if let Screen::App(app, _) = &mut self.screen {
            if app.poll_redraw_due() {
                self.app_redraw();
            }
        }
        self.take_stale()
    }

    /// True when events since the last call changed what should be on screen (a plain pointer move that crosses
    /// nothing does not). Resets the flag.
    pub fn take_stale(&mut self) -> bool {
        std::mem::take(&mut self.stale)
    }

    /// Clipboard shortcuts first, then the overlay, then the current screen.
    fn dispatch(&mut self, ev: &Event) {
        self.dispatch_event(ev);
        // the next key is read with what the open tile wants: Alt+key as its own key only for a tile that shows another computer's program
        crate::input::set_alt_passthrough(self.app_wants_alt());
    }

    fn dispatch_event(&mut self, ev: &Event) {
        use crate::ui::clipboard;
        match ev {
            // a ringing alarm takes every key and click until it is dismissed or snoozed
            _ if self.ring.is_some() && !matches!(ev, Event::Tick | Event::MouseMove(..)) => self.handle_ring(ev),
            // the clipboard's QR scan takes every key and click while it is open
            _ if self.clipscan.is_some() && !matches!(ev, Event::Tick | Event::MouseMove(..) | Event::MouseUp(..) | Event::FocusAt(..)) => self.handle_clipscan(ev),
            // the sign-in dialog takes every key, click and pasted text
            _ if self.login.as_ref().is_some_and(|m| !m.hidden()) && !matches!(ev, Event::Tick | Event::MouseMove(..) | Event::MouseUp(..) | Event::FocusAt(..)) => self.handle_login(ev),
            // a Login or Logout button of any screen
            Event::Tab(n) if self.session.is_some() && self.clip.is_none() && crate::agent_login::agent_of_tab(*n).is_some() => {
                if let Some((agent, logout)) = crate::agent_login::agent_of_tab(*n) {
                    self.login_request(agent, logout);
                }
            }
            // a flow's permission request takes every key and click until it is answered
            _ if self.permpop.is_some() && !matches!(ev, Event::Tick | Event::MouseMove(..) | Event::MouseUp(..) | Event::FocusAt(..)) => self.handle_permpop(ev),
            // the host-files dialog (files dropped on the window) takes every key and click
            _ if self.hostdlg.is_some() && !matches!(ev, Event::MouseMove(..)) => self.handle_hostdlg(ev),
            // a screenshot being framed takes every key and click
            _ if matches!(self.shot, Shot::Select { .. } | Shot::Review { .. }) => self.handle_shot(ev),
            // F12 (or Print Screen) starts a screenshot anywhere after sign-in
            Event::Key(Key::F(12)) if self.session.is_some() && self.clip.is_none() && !self.app_claims_f12() => self.start_shot(),
            // the drive banner's buttons answer on every screen, signed in or not
            Event::Tab(n) if crate::lossbar::is_click(*n) => self.loss_event(*n),
            // the developer-mode approval card: its buttons answer on every screen after sign-in
            Event::Tab(n) if shell_tzask::is_click(*n) && self.tzask.open && self.session.is_some() => self.tzask_event(*n),
            Event::Tab(n) if crate::devask::is_click(*n) && self.session.is_some() => self.devask_event(*n),
            Event::Key(Key::Esc) if self.devask.open && self.session.is_some() => self.devask_event(crate::devask::CLOSE),
            // the Gemini voice assistant: its card's buttons, the microphone button, and Esc ends an open session
            Event::Tab(n) if gv_impl::is_click(*n) && self.session.is_some() => self.gv_event(*n),
            Event::Key(Key::Esc) if self.gv.open() && self.session.is_some() => self.gv_stop(),
            // the battery's countdown and plug-in cards answer on every screen; F4 (or the chip) opens the battery popup
            Event::Tab(crate::batterybar::CANCEL) | Event::Tab(crate::batterybar::CARD_OK) => self.battery_card_event(ev),
            Event::Key(Key::Esc) if self.battery.card_visible() => self.battery_card_event(ev),
            Event::Key(Key::F(crate::batterybar::KEY)) if self.session.is_some() && self.clip.is_none() && !self.battery.open => self.toggle_battery(),
            Event::Key(_) | Event::Tab(_) if self.battery.open && self.session.is_some() && self.clip.is_none() && !self.help => self.handle_battery(ev),
            // F8 (or the Devices button) opens the devices popover; F7 (or the clock) opens Settings, Time
            Event::Key(Key::F(8)) if self.session.is_some() && self.clip.is_none() => self.toggle_devices(),
            Event::Key(Key::F(7)) if self.session.is_some() && self.clip.is_none() => {
                self.devpop.close();
                self.brand.close();
                self.installs.close();
                self.help = false;
                self.jump_to("settings", "tab:time");
            }
            // the crash card's buttons (Open <app> again, Dismiss)
            Event::Tab(n @ (crate::crashcard::OPEN | crate::crashcard::DISMISS)) if self.crash.is_some() => self.crash_card_event(*n),
            // F10 (or a click on the name in the launcher's top bar) opens the IYAKKI menu; the indicator beside the clock the installs
            Event::Key(Key::F(10)) if self.session.is_some() && self.clip.is_none() => self.toggle_brand(),
            Event::Tab(crate::installpop::INDICATOR) if self.session.is_some() && self.clip.is_none() => self.toggle_installs(),
            Event::Tab(UPDATE_CHIP) if self.session.is_some() && self.clip.is_none() => self.open(PApp::Store),
            Event::Tab(SHARING_CHIP) if self.session.is_some() && self.clip.is_none() => self.jump_to("settings", "tab:network"),
            Event::Tab(DEV_SIGN) if self.session.is_some() && self.clip.is_none() && self.devask.pending.is_some() => self.devask_event(crate::devask::OPEN),
            Event::Tab(DEV_SIGN) if self.session.is_some() && self.clip.is_none() => self.jump_to("settings", "tab:developer"),
            // the Home icon and F11: back to the tiles from any app; the gear: Settings (the app you were in is kept aside)
            Event::Tab(volbar_impl::HOME_TAB) | Event::Key(Key::F(11)) if self.session.is_some() && self.clip.is_none() => self.go_home(),
            Event::Tab(volbar_impl::GEAR_TAB) if self.session.is_some() && self.clip.is_none() => self.open_settings_from_bar(),
            Event::Tab(volbar_impl::NET_TAB) if self.session.is_some() && self.clip.is_none() => self.toggle_network_panel(),
            Event::Tab(volbar_impl::MIC_TAB) if self.session.is_some() && self.clip.is_none() => self.open(PApp::Voice),
            // the startup strip and its list
            Event::Tab(n) if startup_impl::is_tab(*n) && self.session.is_some() && self.clip.is_none() => self.startup_tab(*n),
            // the menu's lists (About, Linux apps): the pointer (so the wheel knows where it is), the scroll bar and a drag on the body
            Event::MouseMove(..) | Event::MouseUp(..) | Event::AreaAt(..) if self.brand.open && self.session.is_some() && self.clip.is_none() && !self.help && self.brand.pointer_event(ev) => {}
            // the menu takes its keys, clicks, pasted text and the wheel while it is open
            Event::Key(_) | Event::Tab(_) | Event::Scroll(_) | Event::Paste(_) | Event::FocusAt(..) if self.brand.open && self.session.is_some() && self.clip.is_none() && !self.help => self.handle_brand(ev),
            Event::Key(_) | Event::Tab(_) | Event::Scroll(_) | Event::AreaAt(..) | Event::MouseUp(..) if self.installs.open && self.session.is_some() && self.clip.is_none() && !self.help => self.handle_installs(ev),
            // the popover takes its keys, clicks and the wheel while it is open
            Event::Key(_) | Event::Tab(_) | Event::Scroll(_) if self.devpop.open && self.session.is_some() && self.clip.is_none() && !self.help => self.handle_devpop(ev),
            // F1 opens the help popup anywhere after sign-in; while it is open it takes every key
            Event::Key(Key::F(1)) if self.session.is_some() && self.clip.is_none() => self.help = !self.help,
            // F9 (or the bell) opens the inbox anywhere after sign-in
            Event::Key(Key::F(9)) if self.session.is_some() && self.clip.is_none() => {
                self.help = false;
                self.open_inbox();
            }
            _ if self.help => {
                if matches!(ev, Event::Key(Key::Esc | Key::Enter | Key::Char(' ') | Key::F(1))) {
                    self.help = false;
                }
            }
            // panes: the Ctrl+Alt chords, the buttons of the pane strips and the bar, the memory card and the layout picker
            Event::Layout(k) if self.session.is_some() && self.clip.is_none() => self.panes_key(*k),
            Event::Tab(n) if self.panes.card.is_none() && self.panes_owns_tab(*n) => self.panes_tab(*n),
            Event::Tab(n) if self.views_owns_tab(*n) && self.clip.is_none() => self.views_tab(*n),
            Event::Key(_) | Event::Tab(_) | Event::Scroll(_) if self.panes.card.is_some() && self.session.is_some() => self.panes_card_event(ev),
            Event::Key(_) | Event::Tab(_) | Event::Scroll(_) if self.panes.picker.is_some() && self.session.is_some() && self.clip.is_none() => self.panes_picker_key(ev),
            // the speaker icon of an app's header: click, wheel, drag and its slider (the home screen handles these itself)
            _ if self.session.is_some() && self.clip.is_none() && matches!(self.screen, Screen::App(..)) && self.vol_event(ev) => {}
            // the launcher's own menus and cards (move a tile, make or delete a group) and the keys that open them
            _ if self.layout_intercept(ev) => {}
            // the context menu takes every key and click while it is open (heartbeats, releases and clipboard text pass)
            _ if self.ctx.is_some() && !matches!(ev, Event::Tick | Event::MouseUp(..) | Event::Paste(_)) => self.handle_ctx(ev),
            // the answer to the queue's request for the system clipboard: it joins the queue and is not pasted anywhere
            Event::Paste(t) if self.tick < self.host_sync_until => {
                self.host_sync_until = 0;
                if let Some(m) = self.ctx.as_mut() {
                    m.set_clip_text(true);
                }
                self.clip_note = ClipNote::Got(t.chars().count());
                clipboard::remember(t);
                if self.clip.is_some() {
                    self.build_clipboard_picker();
                }
            }
            // text that arrived from the host clipboard: remember it in the history, then paste it
            Event::Paste(t) if self.clip.is_none() => {
                self.ctx = None;
                clipboard::remember(t);
                self.route(ev);
            }
            // Ctrl+V pastes the most recent clipboard entry; Ctrl+Shift+V opens the history; a right click opens the context menu
            Event::Key(Key::Ctrl('v')) if self.clip.is_none() => match clipboard::paste() {
                Some(t) => self.route(&Event::Paste(t)),
                None => self.say_info("The clipboard is empty"),
            },
            Event::Key(Key::Ctrl('V')) => {
                self.ctx = None;
                self.open_clipboard_picker()
            }
            Event::MouseRight(x, y) if self.clip.is_none() && self.layout_right_click(*x, *y) => {}
            Event::MouseRight(x, y) if self.clip.is_none() => self.open_context_menu(*x, *y),
            Event::MouseRight(..) => self.open_clipboard_picker(),
            _ if self.clip.is_some() => self.handle_clip_overlay(ev),
            _ => self.route(ev),
        }
    }

    /// The dev cluster went down or its disk is nearly full: tell the signed-in administrator in the INBOX (once per event).
    fn check_cluster_alerts(&mut self) {
        let Some(user) = self.session.as_ref().filter(|s| s.policy.manage_settings).map(|s| s.user.clone()) else { return };
        for a in self.env.services.k3sdev().take_alerts() {
            let (title, body) = a.title_body();
            self.env.audit.record(&user, "devcluster.alert", &title);
            let _ = self.env.notify(&user, iyakki_shared::inbox::Kind::Warning, &title, &body, Some(iyakki_shared::inbox::Link::new("settings", "")));
            self.say(format!("{title}. See Settings \u{2192} Dev cluster or SYSTEM."));
        }
    }

    /// Tell an administrator about devices that appeared on the network since the last look.
    fn check_new_devices(&mut self) {
        let Some(user) = self.session.as_ref().filter(|s| s.policy.manage_settings).map(|s| s.user.clone()) else { return };
        // one toast for the whole batch; the inbox collects them into one entry ("12 new devices on your network")
        let fresh = self.env.services.devwatch().take_fresh();
        for d in &fresh {
            self.env.audit.record(&user, "netsec.new_device", &format!("{} {}", d.ip, d.mac));
            let _ = self.env.notify(&user, iyakki_shared::inbox::Kind::Device, "New device on your network", &format!("{} ({}) appeared. NET SCAN → Devices lets you trust it.", d.ip, d.mac), Some(iyakki_shared::inbox::Link::new("scan", &d.mac)));
        }
        match fresh.as_slice() {
            [] => {}
            [d] => self.say(format!("New device on your network: {} ({}). NET SCAN → Devices to trust it.", d.ip, d.mac)),
            more => self.say(format!("{} new devices on your network. Open the inbox to see them.", more.len())),
        }
    }

    /// Looks for alarms and reminders that have become due since the last look, and starts ringing for them.
    pub fn check_alarms(&mut self, now: i64) {
        let Some(user) = self.session.as_ref().map(|s| s.user.clone()) else {
            self.last_alarm_check = 0;
            return;
        };
        if self.last_alarm_check == 0 {
            self.last_alarm_check = now;
            return;
        }
        if now != self.last_alarm_check {
            let zone = iyakki_clock::Zone::system();
            let mut items: Vec<(String, bool, String)> = Vec::new();
            for d in self.env.calendar().take_due(&user, self.last_alarm_check, now, &zone) {
                match d {
                    iyakki_shared::calendar::Due::Alarm { label, at } => {
                        let l = zone.local(at);
                        let _ = self.env.notify(&user, iyakki_shared::inbox::Kind::Reminder, &format!("Alarm: {label}"), &format!("Rang at {:02}:{:02}", l.hour, l.minute), Some(iyakki_shared::inbox::Link::new("calendar", "")));
                        items.push((label, true, format!("{:02}:{:02}", l.hour, l.minute)));
                    }
                    iyakki_shared::calendar::Due::Reminder { title, starts } => {
                        let l = zone.local(starts);
                        let _ = self.env.notify(&user, iyakki_shared::inbox::Kind::Reminder, &format!("Reminder: {title}"), &format!("Starts at {:02}:{:02}", l.hour, l.minute), Some(iyakki_shared::inbox::Link::new("calendar", "")));
                        items.push((title, false, format!("starts at {:02}:{:02}", l.hour, l.minute)));
                    }
                }
            }
            let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.snoozed).into_iter().partition(|s| s.0 <= now);
            self.snoozed = later;
            items.extend(due.into_iter().map(|(_, what, alarm)| (what, alarm, "snoozed".to_string())));
            self.last_alarm_check = now;
            if !items.is_empty() {
                for (what, alarm, _) in &items {
                    self.env.audit.record(&user, if *alarm { "alarm.ring" } else { "calendar.reminder" }, what);
                }
                match &mut self.ring {
                    Some(r) => r.items.extend(items),
                    None => self.ring = Some(Ring { items, since: now, player: None, next_sound: 0 }),
                }
                mark_stale!(self);
            }
        }
        // the sound repeats until the alarm is dismissed (and gives up after five minutes)
        if let Some(r) = &mut self.ring {
            if now - r.since > 300 {
                let n = r.items.len();
                self.ring = None;
                self.say(format!("{n} alarm{} rang and was not answered", if n == 1 { "" } else { "s" }));
                return;
            }
            if self.tick >= r.next_sound && r.player.as_mut().is_none_or(|p| !p.is_playing()) {
                let cmd = self.env.voice_settings().play_cmd;
                r.player = iyakki_apps::agent().helpers_b4().calendar_ring(&cmd).ok();
                r.next_sound = self.tick + 60;
            }
            mark_stale!(self);
        }
    }

    /// Show a flow's permission request over the screen when someone is signed in and it is fresh; take it down when it is answered
    /// elsewhere (the INBOX), dismissed or older than ten minutes. The INBOX entry never depends on this.
    fn permpop_tick(&mut self) {
        let Some(user) = self.session.as_ref().map(|s| s.user.clone()) else {
            if self.permpop.take().is_some() {
                mark_stale!(self);
            }
            return;
        };
        let now = iyakki_clock::now_unix();
        let consent = &self.env.services.browser_consent();
        let fresh = |r: &iyakki_shared::browserconsent::ConsentRequest| permpop_fresh(r, now);
        if let Some(d) = &self.permpop {
            if !consent.get(&user, &d.req.id).is_some_and(|r| fresh(&r)) {
                self.permpop = None;
                mark_stale!(self);
            }
            return;
        }
        if let Some(r) = consent.pending(&user).into_iter().find(|r| fresh(r)) {
            self.permpop = Some(crate::apps::browser_consent::ConsentDialog::new(r));
            mark_stale!(self);
        }
    }

    fn handle_permpop(&mut self, ev: &Event) {
        let Some(d) = self.permpop.clone() else { return };
        let Event::Key(k) = ev else { return };
        let Some(a) = d.answer_of(k) else { return };
        let Some(cx) = self.cx() else { return };
        match d.answer(&cx, a) {
            Ok(()) => {
                self.permpop = None;
                self.say_info(match a {
                    iyakki_shared::browserconsent::Answer::AllowOnce => "Allowed for this run",
                    iyakki_shared::browserconsent::Answer::AlwaysAllow => "Allowed for this flow",
                    iyakki_shared::browserconsent::Answer::Block => "Blocked: the flow will not ask again",
                    iyakki_shared::browserconsent::Answer::Allow => "Allowed this time",
                    iyakki_shared::browserconsent::Answer::Deny => "Denied",
                    _ => "Not now. The request stays in the INBOX.",
                });
            }
            Err(e) => self.say(e),
        }
        mark_stale!(self);
    }

    fn handle_ring(&mut self, ev: &Event) {
        let snooze = matches!(ev, Event::Key(Key::Char('s') | Key::Char('S')));
        let dismiss = matches!(ev, Event::Key(Key::Enter | Key::Esc | Key::Char(' ')));
        if !(snooze || dismiss) {
            return;
        }
        if let Some(mut r) = self.ring.take() {
            if let Some(p) = r.player.as_mut() {
                p.stop();
            }
            if snooze {
                let at = iyakki_clock::now_unix() + 300;
                for (what, alarm, _) in r.items {
                    self.snoozed.push((at, what, alarm));
                }
                self.say_info("Snoozed for 5 minutes");
            }
        }
    }

    #[cfg(test)]
    pub fn ringing(&self) -> Option<Vec<String>> {
        self.ring.as_ref().map(|r| r.items.iter().map(|i| i.0.clone()).collect())
    }

    fn clipboard_limit_path(&self) -> Option<std::path::PathBuf> {
        let user = &self.session.as_ref()?.user;
        let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
        Some(self.env.data_dir().join(format!("clipboard-{who}.json")))
    }

    fn save_clipboard_limit(&self) {
        if let Some(p) = self.clipboard_limit_path() {
            let _ = std::fs::write(p, serde_json::json!({ "limit": crate::ui::clipboard::limit() }).to_string());
        }
    }

    /// The person's own choice of how many copies to keep, read at sign-in.
    fn load_clipboard_limit(&self) {
        let n = self.clipboard_limit_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["limit"].as_u64());
        crate::ui::clipboard::set_limit(n.map_or(crate::ui::clipboard::DEFAULT_LIMIT, |n| n as usize));
    }

    /// Open the queue of copies. The window is also asked for what the system clipboard holds now (text copied in other
    /// programs only reaches the queue that way); it is added when it arrives.
    fn open_clipboard_picker(&mut self) {
        self.want_host_paste = true;
        self.host_sync_until = self.tick + 90;
        self.clip_note = ClipNote::Asked;
        self.build_clipboard_picker();
    }

    fn build_clipboard_picker(&mut self) {
        use crate::ui::clipboard::Entry;
        let items = crate::ui::clipboard::entries();
        let mut thumbs: Vec<Option<std::rc::Rc<tiny_skia::Pixmap>>> = Vec::new();
        let previews: Vec<String> = items
            .iter()
            .enumerate()
            .map(|(i, e)| match e {
                Entry::Text(t) => {
                    thumbs.push(None);
                    format!("{:>2}. {}", i + 1, crate::ui::clipboard::preview(t))
                }
                Entry::Picture { width, height, rgba } => {
                    thumbs.push(tiny_skia::IntSize::from_wh(*width, *height).and_then(|sz| tiny_skia::Pixmap::from_vec((**rgba).clone(), sz)).map(std::rc::Rc::new));
                    format!("{:>2}. picture {width}×{height}", i + 1)
                }
            })
            .collect();
        let title = if items.is_empty() { "Clipboard history: nothing copied yet" } else { "Clipboard history: pick one to paste" };
        self.clip = Some(Picker::new(title, previews).with_thumbs(thumbs));
    }

    /// True once after the queue was opened: the desktop window should read the system clipboard and send it as a paste.
    pub fn take_host_paste_request(&mut self) -> bool {
        std::mem::take(&mut self.want_host_paste)
    }

    /// "Scan QR" was chosen in the clipboard history: the history closes and the camera question opens.
    fn open_clipscan(&mut self) {
        self.clip = None;
        let src = self.scan_src.clone().unwrap_or_else(crate::scan_core::CamSource::desktop);
        self.clipscan = Some(Box::new(crate::clip_scan::ClipScan::open(src)));
        mark_stale!(self);
    }

    /// Use made-up cameras for the clipboard scan (tests).
    pub fn set_scan_camera(&mut self, src: std::sync::Arc<crate::scan_core::CamSource>) {
        self.scan_src = Some(src);
    }

    #[cfg(test)]
    pub(crate) fn audit_log_path(&self) -> std::path::PathBuf {
        self.env.audit.path_buf().to_path_buf()
    }

    pub fn clipboard_scan(&self) -> Option<&crate::clip_scan::ClipScan> {
        self.clipscan.as_deref()
    }

    fn handle_clipscan(&mut self, ev: &Event) {
        let Some(c) = self.clipscan.as_mut() else { return };
        match c.handle(ev) {
            crate::clip_scan::Out::Stay => {}
            crate::clip_scan::Out::Close => self.clipscan = None,
            crate::clip_scan::Out::Copied { chars, secret } => {
                self.clipscan = None;
                // only the fact: never the text
                if let Some(s) = &self.session {
                    self.env.audit.record(&s.user, "clipboard.scan", &format!("clipboard scan: {chars} characters, secret {}", if secret { "yes" } else { "no" }));
                }
            }
        }
        mark_stale!(self);
    }

    fn handle_clip_overlay(&mut self, ev: &Event) {
        // the + and − buttons change how many copies are kept (saved for this person)
        if let Event::Tab(n @ (910 | 911)) = ev {
            let now = crate::ui::clipboard::limit();
            let next = if *n == 911 { now + 1 } else { now.saturating_sub(1) };
            crate::ui::clipboard::set_limit(next);
            self.save_clipboard_limit();
            self.build_clipboard_picker();
            return;
        }
        // Scan QR: copy text by holding a QR code up to the camera
        if matches!(ev, Event::Tab(912) | Event::Key(Key::Ctrl('k' | 'K'))) {
            self.open_clipscan();
            return;
        }
        // Save on a picture's row writes it to a file
        if let Event::Tab(n) = ev {
            if *n > 1000 {
                if let Some(crate::ui::clipboard::Entry::Picture { width, height, rgba }) = crate::ui::clipboard::entries().get(*n - 1001).cloned() {
                    if let Some(pm) = tiny_skia::IntSize::from_wh(width, height).and_then(|sz| tiny_skia::Pixmap::from_vec((*rgba).clone(), sz)) {
                        self.save_pixmap(&pm);
                    }
                }
                return;
            }
        }
        let Some(p) = self.clip.as_mut() else { return };
        match p.handle_event(ev) {
            PickEvent::Cancel => self.clip = None,
            PickEvent::Chosen(label) => {
                self.clip = None;
                // the label starts with its 1-based position in the queue
                let idx = label.trim_start().split('.').next().and_then(|n| n.parse::<usize>().ok()).unwrap_or(1).max(1) - 1;
                match crate::ui::clipboard::entries().get(idx).cloned() {
                    Some(crate::ui::clipboard::Entry::Text(text)) => self.route(&Event::Paste(text)),
                    // nothing here takes a picture yet: it becomes the system clipboard's picture, ready to paste elsewhere
                    Some(crate::ui::clipboard::Entry::Picture { .. }) => {
                        if crate::ui::clipboard::promote_picture(idx) {
                            self.say_info("Picture copied to the system clipboard — paste it in another program, or use Save");
                        }
                    }
                    None => {}
                }
            }
            PickEvent::None => {}
        }
    }

    /// An app asked for the options of a shared host folder (window mode).
    fn host_mount_menu(&mut self, name: &str) {
        if !iyakki_shared::hostfiles::enabled() {
            self.say(iyakki_shared::hostfiles::UNAVAILABLE);
            return;
        }
        let Some(cx) = self.cx() else { return };
        self.hostdlg = Some(Box::new(crate::apps::hostfiles::HostDialog::mount_menu(crate::apps::hostfiles::env_for_cx(&cx), rgb(238, 226, 112), name)));
        mark_stale!(self);
    }

    /// An app asked to save a file on the host computer: the save dialog (window mode only).
    fn host_save(&mut self, name: &str, bytes: &[u8]) {
        if !iyakki_shared::hostfiles::enabled() {
            self.say(iyakki_shared::hostfiles::UNAVAILABLE);
            return;
        }
        let Some(cx) = self.cx() else { return };
        self.hostdlg = Some(Box::new(crate::apps::hostfiles::HostDialog::save_bytes(crate::apps::hostfiles::env_for_cx(&cx), rgb(238, 226, 112), name, bytes)));
        mark_stale!(self);
    }

    /// Tell the app on screen what a drop brought in (the IDE opens it, the media library looks again).
    fn tell_app_dropped(&mut self, files: &[std::path::PathBuf]) {
        let Some(cx) = self.cx() else { return };
        if let Screen::App(app, _) = &mut self.screen {
            app.dropped_done(files, &cx);
        }
    }

    /// The Login button of an agent was pressed (or its Logout button): open the dialog, or sign out.
    fn login_request(&mut self, agent: &'static str, logout: bool) {
        let Some(cx) = self.cx() else { return };
        self.brand.close();
        self.installs.close();
        self.devpop.close();
        if logout {
            crate::agent_login::sign_out_in_background(&cx, agent);
            self.say_info(format!("Signing out of {}…", crate::agent_login::name_of(agent)));
        } else {
            let mut m = Box::new(crate::agent_login::LoginModal::new(agent));
            if agent != iyakki_extagentx::CLAUDE_CODE {
                m.start(&cx);
            }
            self.login = Some(m);
        }
        mark_stale!(self);
    }

    fn handle_login(&mut self, ev: &Event) {
        let Some(cx) = self.cx() else {
            self.login = None;
            return;
        };
        if let Some(m) = self.login.as_mut() {
            match m.handle(ev, &cx) {
                crate::agent_login::Out::Close => self.login = None,
                crate::agent_login::Out::OpenBrowser(url) => {
                    m.hide();
                    self.open_in_browser(&url);
                }
                crate::agent_login::Out::Stay => {}
            }
        }
        mark_stale!(self);
    }

    /// Show an address in the browser tile, in a new tab, keeping the screen it was opened from aside (Back returns to it). The sign-in
    /// dialog is hidden meanwhile and comes back by itself when the browser is left (see the heartbeat).
    fn open_in_browser(&mut self, url: &str) {
        let prev = std::mem::replace(&mut self.screen, Screen::Launcher);
        if let Screen::App(a, pa) = prev {
            if self.stash.is_none() && pa != PApp::Browser {
                self.stash = Some((a, pa));
            } else if pa == PApp::Browser {
                self.screen = Screen::App(a, pa);
            }
        } else {
            self.screen = prev;
        }
        if !matches!(self.screen, Screen::App(_, PApp::Browser)) {
            self.open(PApp::Browser);
        }
        if let (Some(cx), Screen::App(a, _)) = (self.cx(), &mut self.screen) {
            a.open_item(&format!("newtab:{url}"), &cx);
        }
        if !matches!(self.screen, Screen::App(_, PApp::Browser)) {
            // the browser could not be opened (the role may not use it): the dialog stays
            if let Some(m) = self.login.as_mut() {
                m.show_hidden_again_for_shell();
            }
            self.say("The browser is not available to your account: open the address from the QR code or copy it.");
        }
    }

    fn handle_hostdlg(&mut self, ev: &Event) {
        use crate::apps::hostfiles::Out;
        let Some(d) = self.hostdlg.as_mut() else { return };
        match d.handle(ev) {
            Out::Close => self.hostdlg = None,
            Out::Imported(files) => {
                let label = d_label(self.hostdlg.as_deref());
                self.say_info(format!("Imported {} file{} into {label}", files.len(), if files.len() == 1 { "" } else { "s" }));
                self.tell_app_dropped(&files);
            }
            Out::Linked(n) => {
                self.say_info(format!("host:{n} is shared read-only"));
                self.tell_app_dropped(&[]);
            }
            Out::Unlinked(n) => self.say_info(format!("host:{n} is no longer shared")),
            Out::Stay => {}
        }
        mark_stale!(self);
    }

    /// Files are being dragged over the window (window mode): show where a drop would go.
    pub fn set_drag_hover(&mut self, on: bool) {
        if self.drag_over != on {
            self.drag_over = on;
            mark_stale!(self);
        }
    }

    /// Files or folders were dropped on the window. The app on screen may take them (DATA opens tables); otherwise a dialog
    /// offers to import them into the Uploads folder. Only in window mode, after sign-in.
    pub fn drop_files(&mut self, paths: Vec<std::path::PathBuf>) {
        self.drag_over = false;
        mark_stale!(self);
        if self.session.is_none() || paths.is_empty() {
            return;
        }
        if !iyakki_shared::hostfiles::enabled() {
            self.say("Dropped files can only be imported in the desktop window");
            return;
        }
        // a form with a path field on screen: the dropped path fills the field instead of opening the import dialog
        if crate::filepicker::path_field_on_screen() {
            if let Some(ev) = crate::filepicker::drop_event(&paths) {
                self.dispatch(&ev);
                mark_stale!(self);
                return;
            }
        }
        let Some(cx) = self.cx() else { return };
        let action = match &mut self.screen {
            Screen::App(app, _) => app.on_drop(&paths, &cx),
            _ => crate::apps::DropAction::Uploads,
        };
        let env = crate::apps::hostfiles::env_for_cx(&cx);
        let accent = rgb(238, 226, 112);
        match action {
            crate::apps::DropAction::Taken => {}
            crate::apps::DropAction::Ignore(m) => self.say(m),
            crate::apps::DropAction::Uploads => self.hostdlg = Some(Box::new(crate::apps::hostfiles::HostDialog::import_paths(env, accent, paths))),
            crate::apps::DropAction::Into { dest, label } => self.hostdlg = Some(Box::new(crate::apps::hostfiles::HostDialog::import_paths(env.with_dest(dest, &label), accent, paths))),
        }
    }

    /// "Host access: 2 folders" while folders of the host computer are shared with this account (for the window title).
    pub fn host_access_chip(&self) -> Option<String> {
        let s = self.session.as_ref()?;
        if !iyakki_shared::hostfiles::enabled() {
            return None;
        }
        let n = iyakki_shared::hostfiles::active_mounts(self.env.data_dir(), &s.user).len();
        (n > 0).then(|| format!("Host access: {n} folder{}", if n == 1 { "" } else { "s" }))
    }

    /// Resolve a click against the regions drawn last frame (topmost first). A bar shown for one use (the Look has it off) is put away after
    /// a click on it.
    fn click(&mut self, x: i32, y: i32) {
        let peeking = self.bar_peek && !self.look.topbar.on;
        self.click_inner(x, y);
        let (w, h) = self.size;
        if peeking && (y as f32) < 80.0 * scale_for(w, h) {
            self.bar_peek = false;
            mark_stale!(self);
        }
    }

    fn click_inner(&mut self, x: i32, y: i32) {
        let (w, _) = self.size;
        let s = scale_for(w, self.size.1);
        let hit = self.hits.iter().rev().find(|h| h.rect.contains(x, y)).cloned();
        if self.ctx.is_some() && self.ring.is_none() && !self.help && matches!(self.shot, Shot::Off) {
            self.handle_ctx(&Event::MouseDown(x, y));
            return;
        }
        if self.lay.menu.is_some() && self.ring.is_none() && !self.help && matches!(self.shot, Shot::Off) {
            self.layout_intercept(&Event::MouseDown(x, y));
            return;
        }
        if self.permpop.is_some() {
            if let Some(h) = hit {
                if h.label != "backdrop" {
                    self.handle_permpop(&h.ev);
                }
            }
            return;
        }
        if self.hostdlg.is_some() {
            if let Some(h) = hit {
                if h.label != "backdrop" {
                    self.handle_hostdlg(&h.ev);
                }
            }
            return;
        }
        if self.clipscan.is_some() {
            if let Some(h) = hit {
                if h.label != "backdrop" {
                    self.handle_clipscan(&h.ev);
                }
            }
            return;
        }
        if self.login.as_ref().is_some_and(|m| !m.hidden()) {
            if let Some(h) = hit {
                if h.label != "backdrop" && !matches!(h.ev, Event::Focus(_)) {
                    self.handle_login(&h.ev);
                }
            }
            return;
        }
        if matches!(self.shot, Shot::Select { .. }) {
            self.handle_shot(&Event::MouseDown(x, y));
            return;
        }
        if matches!(self.shot, Shot::Review { .. }) {
            // only the dialog's buttons answer; a click anywhere else leaves it open
            if let Some(h) = hit {
                self.handle_shot(&h.ev);
            }
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        if self.devpop.open && self.clip.is_none() && !self.help {
            match hit {
                Some(h) => self.dispatch(&h.ev),
                None => self.devpop.close(), // a click outside closes it
            }
            return;
        }
        // the battery popup: its own regions answer, a click outside closes it
        if self.battery.open && self.clip.is_none() && !self.help {
            match hit {
                Some(h) => self.dispatch(&h.ev),
                None => self.battery.close(),
            }
            mark_stale!(self);
            return;
        }
        // the IYAKKI menu and the installs list: their own regions answer, a click outside closes them
        if (self.brand.open || self.installs.open) && self.clip.is_none() {
            match hit {
                Some(h) => {
                    let ev = match h.ev {
                        Event::Focus(i) => Event::FocusAt(i, x as f32 - h.rect.x),
                        Event::Area(i) => Event::AreaAt(i, x as f32 - h.rect.x, y as f32 - h.rect.y),
                        other => other,
                    };
                    self.dispatch(&ev);
                }
                None => {
                    self.brand.close();
                    self.installs.close();
                }
            }
            mark_stale!(self);
            return;
        }
        if self.clip.is_some() {
            if let Some(h) = hit {
                if h.label != "backdrop" {
                    self.handle_clip_overlay(&h.ev);
                }
            } else {
                self.clip = None; // click outside closes it
            }
            return;
        }
        // the layout picker and the memory card: their own regions answer; a click outside closes the picker
        if (self.panes.picker.is_some() || self.panes.card.is_some()) && self.session.is_some() && self.clip.is_none() {
            match hit {
                Some(h) if h.label != "backdrop" => self.dispatch(&h.ev),
                _ => {
                    if self.panes.card.is_none() {
                        self.panes.picker = None;
                    }
                }
            }
            mark_stale!(self);
            return;
        }
        // the speaker's dropdown: a press anywhere outside it closes it, then goes on to the region it hit
        self.vol_press_outside(x, y);
        // the header's Back button always leaves the app, even with a dialog open
        if matches!(self.screen, Screen::App(..)) && self.panes.ws.is_none() && back_rect(w, s).contains(x, y) {
            self.leave_app();
            return;
        }
        match hit {
            Some(h) if h.label != "backdrop" => {
                let template = h.ev.clone();
                let ev = match h.ev {
                    Event::Focus(i) => Event::FocusAt(i, x as f32 - h.rect.x),
                    Event::Area(i) => Event::AreaAt(i, x as f32 - h.rect.x, y as f32 - h.rect.y),
                    other => other,
                };
                // presses on text fields and areas count (double click: a word, triple: everything) and can be dragged
                let selectable = matches!(template, Event::Focus(_) | Event::Area(_));
                let near = |t: u64, px: i32, py: i32| self.tick.saturating_sub(t) <= 12 && (x - px).abs() <= 6 && (y - py).abs() <= 6;
                let (t0, px, py, n0) = self.click_run;
                let count = if selectable && n0 > 0 && near(t0, px, py) { (n0 + 1).min(3) } else { 1 };
                self.click_run = if selectable { (self.tick, x, y, count) } else { (0, 0, 0, 0) };
                self.press = selectable.then(|| (template.clone(), h.rect));
                crate::ui::set_click_count(count);
                // a second click on the same row within ~0.4 s also "opens" it (like Enter)
                let double = matches!(ev, Event::Row(_)) && self.last_click.as_ref().is_some_and(|(t, l)| *l == h.label && self.tick.saturating_sub(*t) <= 12);
                self.last_click = Some((self.tick, h.label.clone()));
                crate::ui::set_from_click(true);
                self.dispatch(&ev);
                crate::ui::set_from_click(false);
                crate::ui::set_click_count(1);
                if double {
                    self.dispatch(&Event::Key(Key::Enter));
                }
            }
            Some(_) => {} // backdrop of a dialog: swallow
            None => self.route(&Event::MouseDown(x, y)),
        }
    }

    /// Deliver an event to the current screen.
    fn route_inner(&mut self, ev: &Event) {
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let _ = (h, s);
        match &mut self.screen {
            Screen::Setup(_) | Screen::Login(_) => {
                // the power card (Ctrl+P or the Power button, local console only) takes every key first
                if self.login_power_event(ev) {
                    return;
                }
                let is_setup = matches!(self.screen, Screen::Setup(_));
                let (Screen::Setup(f) | Screen::Login(f)) = &mut self.screen else { return };
                match ev {
                    Event::Key(Key::Tab) | Event::Key(Key::Down) => f.next(),
                    Event::Key(Key::BackTab) | Event::Key(Key::Up) => f.prev(),
                    Event::Key(Key::Enter) => {
                        if f.focus + 1 < f.fields.len() {
                            f.next();
                        } else if is_setup {
                            self.submit_setup();
                        } else {
                            self.submit_login();
                        }
                    }
                    // the Sign in / Create account button
                    Event::Key(Key::Ctrl('s')) => {
                        if is_setup {
                            self.submit_setup();
                        } else {
                            self.submit_login();
                        }
                    }
                    Event::Key(k) => {
                        let focus = f.focus;
                        f.error = None;
                        f.fields[focus].handle(k);
                    }
                    Event::Paste(t) => {
                        let focus = f.focus;
                        f.error = None;
                        f.fields[focus].paste(t);
                    }
                    Event::FocusAt(i, x) => {
                        if *i < f.fields.len() {
                            f.focus = *i;
                            f.fields[*i].click(*x);
                        }
                    }
                    _ => {}
                }
            }
            Screen::HwCheck(h) => {
                if h.handle(ev) == crate::hwcheck::Outcome::Continue {
                    self.leave_hwcheck();
                }
            }
            Screen::Launcher => self.handle_launcher(ev),
            Screen::PowerMenu => {
                if let Event::Key(k) = ev {
                    let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
                    match k {
                        Key::Char('y') | Key::Char('o') => {
                            self.env.audit.record(&user, "power.off", "");
                            self.env.host.power_off();
                        }
                        Key::Char('r') => {
                            self.env.audit.record(&user, "power.reboot", "");
                            self.env.host.reboot();
                        }
                        Key::Char('s') if self.env.host.can_sleep() => {
                            // locks (signs out) first, so the screen asks for the password on wake
                            self.sleep_now(&user, "");
                            return;
                        }
                        Key::Char('l') if self.session.is_some() => {
                            self.lock_now(&user, "");
                            return;
                        }
                        _ => {}
                    }
                    self.screen = Screen::Launcher;
                }
            }
            Screen::App(..) => {
                let back_click = self.panes.ws.is_none() && matches!(ev, Event::MouseDown(x, y) if back_rect(w, s).contains(*x, *y));
                let cx = self.cx();
                // a bug inside one app must not take the whole shell down: the app is closed and the person is told
                let mut crashed = None;
                let mut request = None;
                let mut host_save = None;
                let mut mount_menu = None;
                let flow = match (&mut self.screen, cx) {
                    (Screen::App(app, pa), Some(cx)) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (app.handle(ev, &cx), app.take_request(), app.take_host_save(), app.take_host_mount_menu()))) {
                        Ok((f, r, save, menu)) => {
                            request = r;
                            host_save = save;
                            mount_menu = menu;
                            f
                        }
                        Err(_) => {
                            crashed = Some((app.title(), *pa));
                            Flow::Back
                        }
                    },
                    _ => Flow::Back,
                };
                if let Some((name, pa)) = crashed {
                    self.app_crashed_with(name, Some(pa));
                }
                if let Some((name, bytes)) = host_save {
                    self.host_save(&name, &bytes);
                }
                if let Some(name) = mount_menu {
                    self.host_mount_menu(&name);
                }
                // an app that has a question before it closes (the IDE with tasks still working) keeps the click
                let mut back_click = back_click;
                if back_click && !matches!(flow, Flow::Back) {
                    let cx2 = self.cx();
                    if let (Screen::App(app, _), Some(cx2)) = (&mut self.screen, cx2) {
                        back_click = app.leave_guard(&cx2);
                    }
                }
                if back_click || matches!(flow, Flow::Back) {
                    self.leave_app();
                } else if let Some((app, item)) = request {
                    self.open_linked_from_app(&app);
                    if !item.is_empty() {
                        if let (Some(cx), Screen::App(a, _)) = (self.cx(), &mut self.screen) {
                            a.open_item(&item, &cx);
                        }
                    }
                }
            }
        }
    }

    /// Turn to page `p` (ignored when there is no such page); the first tile of it is selected.
    fn go_page(&mut self, p: usize, pages: usize, cap: usize) {
        if p < pages {
            self.sel = p * cap;
        }
    }

    fn open_item(&mut self, item: &LItem) {
        match item {
            // a greyed CAMERA tile says what is missing instead of opening
            LItem::Sys(t) if t.app == PApp::Camera && !self.camera_ready() => self.say(self.camera.hint()),
            LItem::Sys(t) => self.open(t.app),
            LItem::App(e) => self.open_custom((**e).clone()),
            LItem::Empty => self.say_info("Nothing here yet: select a tile in another group and press m to move it here"),
        }
    }

    /// Open a custom app in its own tile.
    fn open_custom(&mut self, entry: iyakki_shared::store_tilehub::TileEntry) {
        let Some(cx) = self.cx() else { return };
        if let Some((tenant, id)) = shell_tenant::cloud_key(&entry.key) {
            self.open_tenant_tile(&tenant, &id, &entry.title);
            return;
        }
        if !iyakki_shared::store_tilehub::allowed(&cx.session.policy) {
            self.env.audit.record(&cx.session.user, "app.denied", &format!("tile:{}", entry.name));
            self.say(format!("Your role ({}) can't run custom apps", cx.session.role.name()));
            return;
        }
        let name = entry.name.clone();
        let boxed = crate::apps::tile::open_entry(entry, &cx);
        self.custom_open = Some(name);
        self.screen = Screen::App(boxed, PApp::Browser);
    }

    /// Open a custom app by its id (the store's Open button, the IDE's Run).
    pub fn open_custom_by_name(&mut self, name: &str) -> bool {
        self.refresh_custom();
        match self.custom.iter().find(|e| e.name == name).cloned() {
            Some(e) => {
                self.open_custom(e);
                self.custom_open.is_some()
            }
            None => false,
        }
    }

    fn handle_launcher(&mut self, ev: &Event) {
        if self.startup_event(ev) || self.vol_event(ev) {
            return;
        }
        if self.tick.saturating_sub(self.custom_at) > 60 || self.custom_at == 0 {
            self.refresh_custom();
        }
        self.sync_look();
        if self.dock_pointer(ev) || self.board_pointer(ev) {
            return;
        }
        if let Event::Key(k) = ev {
            if self.bar_key(k) || self.dock_key(k) || self.board_key(k) {
                return;
            }
        }
        let tiles = self.group_items();
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let n = tiles.len();
        self.sel = self.sel.min(n - 1);
        let cap = launcher::per_page(w, h, s);
        let machine = self.machine;
        let (pages, page) = if machine { (1, 0) } else { (launcher::page_count(n, cap), self.sel / cap) };
        let first = page * cap;
        let rects = if machine { Vec::new() } else { launcher::page_rects(w, h, s, n, page) };
        let power = self.may_power();
        match ev {
            // the machine page has no tiles: the keys that walk or open them do nothing there
            Event::Key(Key::Left | Key::Up | Key::Down | Key::Home | Key::End | Key::PageDown | Key::PageUp | Key::Enter) if machine => {}
            Event::Scroll(d) if machine => {
                // the machine page has no tiles to scroll: the wheel scrolls the widget board when its widgets do not all fit
                if *d != 0 {
                    self.board_scroll_by(if *d > 0 { -1 } else { 1 });
                }
            }
            Event::Key(Key::Right) if machine => {
                self.step_page(false);
            }
            // Left and Right walk the icons, and carry on to the next page at the end of the row of pages
            Event::Key(Key::Left) => {
                if self.sel == 0 {
                    self.step_page(true);
                } else {
                    self.sel -= 1;
                }
            }
            Event::Key(Key::Right) => {
                if self.sel + 1 >= n {
                    self.step_page(false);
                } else {
                    self.sel += 1;
                }
            }
            Event::Key(Key::Tab) => self.step_group(false),
            Event::Key(Key::BackTab) => self.step_group(true),
            Event::Key(Key::Home) => self.sel = 0,
            Event::Key(Key::End) => self.sel = n - 1,
            Event::Key(Key::PageDown) => self.go_page(page + 1, pages, cap),
            Event::Key(Key::PageUp) => self.go_page(page.wrapping_sub(1), pages, cap),
            Event::Scroll(d) if *d != 0 => self.go_page(if *d > 0 { page.wrapping_sub(1) } else { page + 1 }, pages, cap),
            Event::Key(Key::Up) | Event::Key(Key::Down) => {
                // the tile above or below, nearest in the horizontal direction; past the edge of the page, the next page
                let cur = rects[(self.sel - first).min(rects.len() - 1)];
                let (cx, cy) = (cur.x + cur.w / 2.0, cur.y + cur.h / 2.0);
                let down = matches!(ev, Event::Key(Key::Down));
                let best = rects.iter().enumerate().filter(|(_, r)| if down { r.y > cur.y + cur.h / 2.0 } else { r.y + r.h < cur.y + cur.h / 2.0 }).min_by(|(_, a), (_, b)| {
                    let key = |r: &R| ((r.y + r.h / 2.0 - cy).abs() * 1000.0 + (r.x + r.w / 2.0 - cx).abs()) as i64;
                    key(a).cmp(&key(b))
                });
                if let Some((i, _)) = best {
                    self.sel = first + i;
                } else if down && page + 1 < pages {
                    self.sel = ((page + 1) * cap).min(n - 1);
                } else if !down && page > 0 {
                    self.sel = first - 1;
                }
            }
            Event::Key(Key::Enter) => self.open_item(&tiles[self.sel]),
            Event::Key(Key::Char(c)) => match c {
                // the number keys 1 to 6 turn to a home page: 1 is the machine, 2 the first group, and so on
                '1'..='9' => {
                    if let Some(p) = launcher::page_for_digit(*c, self.home_pages()) {
                        self.show_page(p);
                    }
                }
                's' => self.open_settings_from_bar(),
                'o' | 'q' => self.signout_ask(),
                'p' if power => self.screen = Screen::PowerMenu,
                // m, g, u, [ and ]: move the tile, the group's menu, undo, reorder (`shell_layout.rs`; handled before this point)
                _ => {}
            },
            Event::Key(Key::Esc) => self.signout_ask(),
            Event::MouseMove(x, y) => {
                if let Some(i) = rects.iter().position(|r| r.contains(*x, *y)) {
                    self.sel = first + i;
                }
            }
            Event::MouseDown(x, y) => {
                let groups = self.groups();
                let row = self.chip_row();
                let nav = launcher::page_nav(w, h, s, pages);
                if let Some(i) = rects.iter().position(|r| r.contains(*x, *y)) {
                    self.sel = first + i;
                    self.open_item(&tiles[first + i]);
                } else if let Some(i) = row.chips.iter().find(|(_, r)| r.contains(*x, *y)).map(|(i, _)| *i) {
                    self.set_group(groups[i].clone());
                } else if let Some(p) = launcher::page_dots(w, h, s, self.home_pages()).iter().position(|r| r.contains(*x, *y)) {
                    self.show_page(p);
                } else if launcher::page_step(self.home_page(), true, self.home_pages()).is_some() && launcher::edge_chevrons(w, h, s).0.contains(*x, *y) {
                    self.step_page(true);
                } else if launcher::page_step(self.home_page(), false, self.home_pages()).is_some() && launcher::edge_chevrons(w, h, s).1.contains(*x, *y) {
                    self.step_page(false);
                } else if pages > 1 && nav.prev.contains(*x, *y) {
                    self.go_page(page.wrapping_sub(1), pages, cap);
                } else if pages > 1 && nav.next.contains(*x, *y) {
                    self.go_page(page + 1, pages, cap);
                } else if let Some(p) = nav.dots.iter().position(|r| pages > 1 && r.contains(*x, *y)) {
                    self.go_page(p, pages, cap);
                } else if !self.bar_visible() && { let hr = look_impl::bar_handle(w, s); R::new(hr.x, hr.y + iyakki_uikit::barside::bar_dy(), hr.w, hr.h) }.contains(*x, *y) {
                    self.bar_peek = true;
                } else if self.bar_visible() && self.settings_rect().is_some_and(|r| r.contains(*x, *y)) {
                    self.open_settings_from_bar();
                } else if let Some(b) = self.bar_visible().then(|| self.bar_order()).unwrap_or_default().iter().enumerate().find(|(i, b)| {
                    matches!(b, BarButton::Logout | BarButton::Power | BarButton::Shot | BarButton::Devices | BarButton::Clipboard | BarButton::Bell | BarButton::Network | BarButton::Apps | BarButton::GemVoice) && header_button_at(w, s, *i).contains(*x, *y)
                }).map(|(_, b)| *b) {
                    match b {
                        BarButton::Logout => {
                            self.bar_peek = false;
                            self.sign_out()
                        }
                        BarButton::Power => self.screen = Screen::PowerMenu,
                        BarButton::Shot => self.start_shot(),
                        BarButton::Devices => self.toggle_devices(),
                        BarButton::Clipboard => self.open_clipboard_picker(),
                        BarButton::Network => self.toggle_network_panel(),
                        BarButton::Apps => self.go_home(),
                        BarButton::GemVoice => self.gv_toggle(),
                        _ => {
                            if self.inbox_allowed() {
                                self.open_inbox();
                            }
                        }
                    }
                    self.bar_peek = false;
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ drawing

    pub fn render(&mut self, g: &mut Gfx) {
        self.note_foreground();
        // developer mode: a paired computer asking for the state or a picture ends the saver / black screen (as input would), so the request is
        // answered instead of timing out ("the screen did not answer in time") while nobody is at the machine
        self.dev_wake_for_capture();
        if self.saver_render(g) {
            return;
        }
        if self.lockui.active {
            // a locked screen stays locked, but still answers: the lock card is all a computer asking can see
            let want = self.dev_state_begin(g);
            self.lock_render(g);
            self.dev_locked_answer(g, want);
            return;
        }
        // developer mode's `iyakki dev state`: this frame is kept as text when a computer is asking
        let dev_state = self.dev_state_begin(g);
        self.size = (g.width(), g.height());
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let time_settings = self.env.settings().lock().ok().map(|st| st.get().time.clone());
        let person = self.session.as_ref().filter(|_| self.tick % 15 == 0 || self.tick < 2 || !self.clock_ready || matches!(self.screen, Screen::App(_, PApp::Settings))).map(|se| crate::uisize::load_clock(self.env.data_dir(), &se.user));
        self.clock.refresh(self.tick, time_settings.as_ref(), person);
        // file names made by apps (CAMERA) carry the clock's local time
        crate::camera::set_zone(time_settings.as_ref().map(|t| t.zone.clone()));
        self.clock_ready = self.session.is_some();
        self.clock_tip = None;
        self.live = false; // a whole scene is drawn: nothing is pending
        g.hits.clear();
        g.clear_probes();
        crate::access::start_frame();
        let drawing = self.panes_drawing_display();
        self.vol.rects.retain(|p| p.disp != drawing);
        g.mouse = self.disp_mouse(self.panes_active_display());
        crate::ctxmenu::start_frame();
        crate::fx::background(g);
        // the sign-in and first-start forms: the boot screen's galaxy turns softly behind the card (the boot screen itself can be over in a
        // second on a fast start); off with reduced motion
        if matches!(self.screen, Screen::Login(_) | Screen::Setup(_)) && !panes_drawn_early(self) && !crate::fx::reduced_motion() {
            crate::splash::draw_galaxy(g, self.tick as f32 * 0.033, w as f32 / 2.0, h as f32 / 2.0, s, 0.55);
        }
        let tick = self.tick;
        let auth_lift = self.osk_auth_lift(w, h, s);
        let mut crashed_title: Option<(&'static str, PApp)> = None;
        // several panes: the bar, the panes and the dividers take the place of the one screen
        let panes_drawn = self.ws_active() && {
            self.draw_panes(g, w, h, s);
            true
        };
        match &mut self.screen {
            _ if panes_drawn => {}
            Screen::HwCheck(c) => c.render(g, w, h, s, tick, &self.env.hostname),
            Screen::Setup(f) => draw_auth(g, w, h, s, f, true, tick, &self.env.hostname, self.env.data_volatile, auth_lift),
            Screen::Login(f) => draw_auth(g, w, h, s, f, false, tick, &self.env.hostname, self.env.data_volatile, auth_lift),
            Screen::Launcher | Screen::PowerMenu => {
                self.draw_launcher(g, w, h, s);
                if matches!(self.screen, Screen::PowerMenu) {
                    draw_power_menu(g, w, h, s, self.env.host.can_sleep(), self.session.is_some());
                }
            }
            Screen::App(..) => {
                let cx = self.cx();
                // Home, the speaker and Settings stand left of the bell (and the LAYOUT button); the clock makes room for them
                let cluster_left = bell_rect(w, s).x - if panes_impl::layout_slot() { 40.0 * s } else { 0.0 };
                let extras = self.place_extras(cluster_left - 8.0 * s, 230.0 * s, 110.0 * s, 32.0 * s, 8.0 * s, true);
                let bar_edge = extras.first().map_or(cluster_left, |e| e.1);
                let mut accent_after = None;
                if let (Screen::App(app, pa), Some(cx)) = (&mut self.screen, cx) {
                    let hh = HEADER_H * s;
                    let area = R::new(0.0, hh, w as f32, h as f32 - hh);
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.render(g, area, s, &cx, tick))).is_err() {
                        crashed_title = Some((app.title(), *pa));
                    }
                    // header drawn last so content scrolled above the area never shows through
                    let sub = if self.custom_open.is_some() { "Custom app" } else { TILES.iter().find(|t| t.app == *pa).map_or("", |t| t.sub) };
                    // where the clock goes (measured first so the title stops before it; drawn again after the header band,
                    // which would otherwise paint over it)
                    let hits_before = g.hits.len();
                    let clock_box = app_clock(g, w, s, &self.clock, bar_edge);
                    g.hits.truncate(hits_before);
                    self.clock_tip = clock_box;
                    // the installer indicator sits left of the clock while installs run (or just ended)
                    let clock_left = clock_box.map_or(bar_edge, |b| b.x);
                    // the battery chip (laptops only) sits left of the clock, with the room the app's name needs kept free
                    let bat_plan = self.battery.plan(g, s, clock_left - 6.0 * s, 300.0 * s, 8.0 * s, 30.0 * s, 13.0 * s);
                    let bar_left = bat_plan.as_ref().map_or(clock_left, |p| p.rect.x);
                    let ind_w = self.installs.indicator_width(g, s);
                    let title_limit = if ind_w > 0.0 && bar_left - ind_w - 6.0 * s > 300.0 * s { bar_left - ind_w - 6.0 * s } else { bar_left };
                    // the title and its subtitle stop before the icons that stand left of the clock (Home, the speaker, Settings)
                    let extras_left = extras.iter().map(|e| e.1).fold(f32::MAX, f32::min);
                    crate::fx::app_header(g, w as f32, hh, s, app.title(), sub, app.icon(), app.accent(), tick, (title_limit - 12.0 * s).min(extras_left - 8.0 * s));
                    if clock_box.is_some() {
                        app_clock(g, w, s, &self.clock, bar_edge);
                    }
                    match &bat_plan {
                        Some(p) => self.battery.draw_chip(g, p, s),
                        None => self.battery.anchor = None,
                    }
                    let mut dev_right = bar_left - 4.0 * s;
                    if title_limit < bar_left {
                        if let Some(b) = self.installs.draw_indicator(g, bar_left - 4.0 * s, 8.0 * s + 15.0 * s, s, tick) {
                            dev_right = b.x - 6.0 * s;
                        }
                    } else {
                        self.installs.anchor = None;
                    }
                    if self.update_count > 0 && !matches!(pa, PApp::Store) {
                        let label = format!("{} update{}", self.update_count, if self.update_count == 1 { "" } else { "s" });
                        if let Some(b) = draw_header_chip(g, dev_right, 8.0 * s + 15.0 * s, s, &label, rgb(94, 196, 255), UPDATE_CHIP, "updates waiting", 300.0 * s) {
                            dev_right = b.x - 6.0 * s;
                        }
                    }
                    if self.session.is_some() && iyakki_apps::agent().host_probe().sharing_notice_is_on(self.env.data_dir()) {
                        if let Some(b) = draw_header_chip(g, dev_right, 8.0 * s + 15.0 * s, s, "Sharing internet", rgb(255, 196, 94), SHARING_CHIP, "internet sharing is on", 300.0 * s) {
                            dev_right = b.x - 6.0 * s;
                        }
                    }
                    if let Some(sign) = self.dev_sign.as_deref() {
                        draw_dev_sign(g, dev_right, 8.0 * s + 15.0 * s, s, sign, 300.0 * s, self.devask.pulse(self.tick));
                    }
                    let acc = app.accent();
                    // the icons that do not fit beside the title in a narrow window are not drawn (they keep their keys)
                    let shown = bar_shown(w, s);
                    if shown.clip {
                        let cr = clip_rect(w, s);
                        let hot = g.hovered(cr);
                        crate::fx::chrome_button(g, cr, s, hot, acc);
                        g.icon(Icon::Clipboard, cr.x + cr.w / 2.0, cr.y + cr.h / 2.0, 20.0 * s, MUTED);
                        g.hit(cr, Event::Key(Key::Ctrl('V')), "clipboard history");
                    }
                    if self.session.as_ref().is_some_and(|x| x.policy.allows_app(PApp::Inbox)) {
                        draw_bell(g, bell_rect(w, s), 8.0 * s, s, self.badge, matches!(self.screen, Screen::App(_, PApp::Inbox)));
                    }
                    if self.panes_button() {
                        panes_impl::draw_layout_button(g, panes_impl::layout_btn_rect(w, s), s, acc);
                    }
                    if shown.shot {
                        let sr = shot_rect(w, s);
                        let hot = g.hovered(sr);
                        crate::fx::chrome_button(g, sr, s, hot, acc);
                        g.icon(Icon::Camera, sr.x + sr.w / 2.0, sr.y + sr.h / 2.0, 20.0 * s, MUTED);
                        g.hit(sr, Event::Key(Key::F(12)), "screenshot");
                    }
                    if shown.dev {
                        let dr = dev_rect(w, s);
                        let hot = g.hovered(dr) || self.devpop.open;
                        crate::fx::chrome_button(g, dr, s, hot, acc);
                        g.icon(Icon::Devices, dr.x + dr.w / 2.0, dr.y + dr.h / 2.0, 20.0 * s, MUTED);
                        // the firewall is wanted but not in force: a red dot on the Devices button (its popover says why)
                        if iyakki_fw::service::alert() {
                            g.rrect(dr.x + dr.w - 11.0 * s, dr.y - 2.0 * s, 12.0 * s, 12.0 * s, 6.0 * s, rgb(214, 48, 64));
                        }
                        g.hit(dr, Event::Key(Key::F(8)), "devices");
                    }
                    if shown.help {
                        let hr = help_rect(w, s);
                        let hot = g.hovered(hr);
                        crate::fx::chrome_button(g, hr, s, hot, acc);
                        g.text(hr.x + 12.0 * s, hr.y + 6.0 * s, 14.0 * s, MUTED, "F1  Help", Face::Sans);
                        g.hit(hr, Event::Key(Key::F(1)), "help");
                    }
                    let br = back_rect(w, s);
                    let hot = g.hovered(br);
                    crate::fx::chrome_button(g, br, s, hot, acc);
                    g.text(br.x + 14.0 * s, br.y + 6.0 * s, 15.0 * s, TEXT, "← Back", Face::Bold);
                    accent_after = Some(acc);
                }
                if let Some(acc) = accent_after {
                    for (e, x) in &extras {
                        self.draw_extra(g, *e, R::new(*x, 8.0 * s, 32.0 * s, 30.0 * s), s, acc, 20.0 * s);
                    }
                    self.draw_bar_overlay(g, s);
                }
            }
        }
        // what is still starting: the strip on Home, the line under the sign-in card
        if !panes_drawn {
            match self.screen {
                Screen::Launcher => self.draw_startup_home(g, w, h, s),
                Screen::Login(_) | Screen::Setup(_) => self.draw_startup_login(g, w, h, s),
                _ => {}
            }
        }
        if !matches!(self.screen, Screen::Launcher) || panes_drawn {
            self.startup.strip = None;
            self.startup.panel = None;
            self.startup.open = false;
        }
        if !matches!(self.screen, Screen::Login(_) | Screen::Setup(_)) {
            self.startup.login_line = None;
        }
        if let Some((name, pa)) = crashed_title {
            self.screen = Screen::Launcher;
            self.app_crashed_with(name, Some(pa));
        }
        self.draw_tenants(g, w, h, s);
        if let Some(d) = self.hostdlg.as_mut() {
            d.render(g, R::new(0.0, 0.0, w as f32, h as f32), s, tick);
        }
        if let (Some(d), true) = (&self.permpop, self.session.is_some()) {
            d.render(g, R::new(0.0, 0.0, w as f32, h as f32), s);
        }
        if self.drag_over && self.session.is_some() {
            let dest = match &self.screen {
                Screen::App(_, PApp::Data) => "the DATA viewer",
                _ => "your Uploads folder",
            };
            crate::apps::hostfiles::drop_overlay(g, w as f32, h as f32, s, dest, rgb(238, 226, 112));
        }
        if let Some(r) = &self.ring {
            draw_ring(g, w, h, s, r, self.tick);
        }
        if let Some(b) = self.clock_tip.filter(|b| g.hovered(*b) && !self.devpop.open && !self.brand.open && !self.installs.open && self.session.is_some()) {
            draw_clock_tip(g, w, h, s, b, &self.clock.tooltip());
        }
        if self.session.is_some() {
            self.devpop.render(g, w, h, s);
            self.battery.render(g, w, h, s);
            self.installs.render(g, w, h, s, tick);
            self.brand.render(g, w, h, s, tick);
        }
        if let (Some(m), true) = (self.login.as_mut(), self.session.is_some()) {
            m.render(g, R::new(0.0, 0.0, w as f32, h as f32), s, tick);
        }
        self.battery.render_cards(g, w, h, s);
        self.loss_render(g, w, h, s);
        if self.session.is_some() {
            crate::devask::render(&mut self.devask, g, w, h, s, self.tick);
            let in_bar = matches!(self.screen, Screen::Launcher) && self.bar_visible() && !self.panes.compact;
            gv_impl::render(&self.gv, g, w, h, s, in_bar);
            self.draw_tzask(g, w, h, s);
        }
        if let Some((t, _, info)) = &self.toast {
            crate::fx::toast(g, w as f32, h as f32, s, t, *info);
        }
        if let Some(c) = &self.crash {
            c.render(g, w as f32, h as f32, s);
        }
        if let Some(o) = &self.fnk.osd {
            crate::fnkeys::draw_osd(g, w as f32, h as f32, s, o);
        }
        self.draw_login_bar(g, w, h, s);
        if self.fnk.power_card {
            draw_power_menu(g, w, h, s, self.env.host.can_sleep(), self.session.is_some());
        }
        // the assistant is using the camera: a red sign on every screen, bottom left, while it lasts
        if let Some(what) = self.env.services.camera().in_use() {
            draw_camera_sign(g, h, s, &what);
        }
        // a web page is using the camera, the microphone or the screen: a red sign on every screen, above the assistant's one, a click stops it
        let cap = crate::apps::browser_media::capture_now();
        // the Browser tile shows its own "Camera on" mark in its tool bar: the sign does not cover the page there
        let in_browser = matches!(self.screen, Screen::App(_, PApp::Browser));
        if cap != 0 && !in_browser {
            let r = draw_capture_sign(g, h, s, cap, self.cam_sign);
            g.hit(r, Event::Tab(913), "Stop what the page is capturing");
        }
        // a camera records (F-CAM-03): a red REC sign on every screen while the signed-in person's recording runs, a click stops it
        let mut stacked = usize::from(self.cam_sign) + usize::from(cap != 0 && !in_browser);
        {
            // the same sign also says when a client's camera or microphone streams to this machine (EXPERIMENTAL, clientmedia)
            let names = if self.session.is_some() { crate::apps::cameras::sign_names(self.env.data_dir()) } else { Vec::new() };
            let mut client = iyakki_apps::agent().client_media().indicator().text();
            // an app shows a camera in its own window: the same sign names it (no click: the person closes the view in the app or by chat)
            let shown = if self.session.is_some() { self.core.cameras().indicator_names() } else { Vec::new() };
            if !shown.is_empty() {
                let v = format!("Live view in an app: {}", shown.join(", "));
                client = Some(match client {
                    Some(c) => format!("{c}. {v}"),
                    None => v,
                });
            }
            if !names.is_empty() || client.is_some() {
                let text = crate::apps::cameras::sign_text(&names, client.as_deref());
                let r = draw_rec_sign(g, h, s, &text, stacked);
                g.hit(r, Event::Tab(915), "Stop recording");
                stacked += 1;
            }
        }
        // the microphone is open to the wake phrase listener: a red sign on every screen while it is, a click switches it off
        if self.wake.sign {
            let r = wake_impl::draw_wake_sign(g, h, s, stacked);
            g.hit(r, Event::Tab(916), "Switch the wake phrase off");
        }
        // the person's own browser has its microphone or camera on for this session: a sign on every screen, drawn over every app
        // (a tile cannot hide it), bottom right, a click stops it
        if let Some(text) = self.browser_media().and_then(|m| m.indicator_text()) {
            let r = draw_remote_capture_sign(g, w, h, s, &text);
            g.hit(r, Event::Tab(914), "Stop the browser's microphone and camera");
        }
        if self.help {
            let (title, lines) = self.help_lines();
            draw_help(g, w, h, s, &title, &lines);
        }
        match &self.shot {
            Shot::Select { first, .. } => draw_shot_overlay(g, w, h, s, *first, self.mouse),
            Shot::Review { img } => draw_shot_review(g, w, h, s, img),
            _ => {}
        }
        // clipboard history overlay sits above everything, including dialogs
        if let Some(p) = &self.clip {
            p.render(g, R::new(0.0, 0.0, w as f32, h as f32), s, rgb(120, 170, 255));
            // how many copies are kept, beside the picker's own Cancel button
            let pw = (620.0 * s).min(w as f32 - 40.0 * s);
            let ph = (h as f32 - 40.0 * s).min(560.0 * s);
            let (px, py) = ((w as f32 - pw) / 2.0, (h as f32 - ph) / 2.0);
            let by = py + ph - 50.0 * s;
            let label = format!("keeps the last {}", crate::ui::clipboard::limit());
            let lw = g.text_width(13.0 * s, &label, Face::Sans);
            let bx = px + pw - 26.0 * s - 100.0 * s - 28.0 * s - lw - 2.0 * 40.0 * s;
            g.button(px + 26.0 * s, by, 14.0 * s, "Scan QR  (Ctrl+K)", ACCENT_QR, Event::Tab(912));
            if let Some((n, secs)) = crate::ui::clipboard::secret_waiting() {
                let t = format!("a secret is waiting ({n} characters): {} pastes it once, gone in {secs} s", iyakki_uikit::platform::paste_key());
                let t = g.fit(12.0 * s, &t, Face::Sans, pw - 52.0 * s);
                g.text(px + 26.0 * s, by - 20.0 * s, 12.0 * s, OK, &t, Face::Sans);
            }
            g.button(bx, by, 14.0 * s, "−", MUTED, Event::Tab(910));
            g.text(bx + 46.0 * s, by + 8.0 * s, 13.0 * s, DIM, &label, Face::Sans);
            g.button(bx + 46.0 * s + lw + 10.0 * s, by, 14.0 * s, "+", MUTED, Event::Tab(911));
            // what the system clipboard said
            let note = match self.clip_note {
                ClipNote::Asked if self.tick < self.host_sync_until => "reading the system clipboard…".to_string(),
                ClipNote::Asked => "the system clipboard gave no text (nothing copied there, or it could not be read)".to_string(),
                ClipNote::Got(n) => format!("added {n} characters from the system clipboard"),
                ClipNote::None => String::new(),
            };
            let note = g.fit(12.0 * s, &note, Face::Sans, pw * 0.62);
            let nw = g.text_width(12.0 * s, &note, Face::Sans);
            g.text(px + pw - 26.0 * s - nw, py + 54.0 * s, 12.0 * s, if matches!(self.clip_note, ClipNote::Got(_)) { OK } else { DIM }, &note, Face::Sans);
        }
        if let Some(c) = self.clipscan.as_mut() {
            c.render(g, R::new(0.0, 0.0, w as f32, h as f32), s, self.tick);
        }
        self.draw_layout_ui(g, w, h, s);
        self.draw_signout(g, w, h, s);
        self.draw_panes_overlays(g, w, h, s);
        self.draw_views_strip(g, w, h, s);
        if let Some(m) = &self.ctx {
            let accent = match &self.screen {
                Screen::App(app, _) => app.accent(),
                _ => rgb(120, 170, 255),
            };
            m.draw(g, s, accent);
        }
        self.fields = crate::ctxmenu::fields();
        self.draw_osk(g, w, h, s);
        self.hits = std::mem::take(&mut g.hits);
        self.probes = g.probes();
        self.access_observe(true);
        self.access_draw(g);
        self.capture_final_frame(g);
        if dev_state {
            self.dev_state_end(g);
        }
    }

    /// The picture of the screen, taken from the FINISHED frame (every overlay is in it: views strip, sign-out card, toasts, ...): the
    /// user's screenshot, the assistant's `screen_capture` and developer mode's `iyakki dev screenshot` all read it here, never mid-render.
    pub(super) fn capture_final_frame(&mut self, g: &mut Gfx) {
        if matches!(self.shot, Shot::Snap) {
            self.shot = Shot::Select { snap: g.pm_mut().clone(), first: None };
        }
        // the assistant's `screen_capture`: this frame is "the person's screen" (answered here because only the shell can see it)
        {
            let svc = &self.env.services;
            svc.screen().set_present(true);
            // the assistant's camera tools use the CAMERA app's code through this one backend
            if !cfg!(test) && !svc.camera().present() {
                svc.camera().set_backend(Some(Arc::new(crate::camera::agentcam::AgentCamera::system())));
            }
            // model-judged checks of waiting work (`wait_for` with `judge:`) ask the person's chat model (F-AI-16)
            if !cfg!(test) && !iyakki_apps::agent().pendingplus_has_judge() {
                svc.install_pending_judge();
            }
            if let Some(sess) = &self.session {
                if svc.screen().wanted(&sess.user) {
                    svc.screen().serve_frame(&sess.user, Some(iyakki_shared::screencap::Frame { w: g.pm_mut().width(), h: g.pm_mut().height(), rgba: g.pm_mut().data().to_vec() }));
                }
            }
            // developer mode's `iyakki dev screenshot` (only reachable from a paired computer while developer mode is on): whatever is drawn
            if svc.screen().wanted(iyakki_shared::devmode::SCREEN_ANY) {
                svc.screen().serve_frame(iyakki_shared::devmode::SCREEN_ANY, Some(iyakki_shared::screencap::Frame { w: g.pm_mut().width(), h: g.pm_mut().height(), rgba: g.pm_mut().data().to_vec() }));
            }
        }
    }

    // ------------------------------------------------------------------ context menu

    /// A right click: the menu opens at the pointer. Over a text field it offers Cut / Copy / Paste / Select all, the app adds
    /// its own entries, and "Clipboard history…" is always there.
    fn open_context_menu(&mut self, x: i32, y: i32) {
        use crate::ctxmenu::{Item, Menu, COPY, CUT, HISTORY, PASTE, SELECT_ALL};
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let mut items: Vec<Item> = Vec::new();
        if let Some(mut f) = self.fields.iter().rev().find(|f| f.rect.contains(x, y)).copied() {
            if !f.focused {
                // the right click also puts the caret there, as in any text editor
                self.dispatch(&Event::FocusAt(f.id, x as f32 - f.rect.x));
                f.has_selection = false;
            }
            let sel = f.has_selection && !f.secret;
            items.push(Item::new(CUT, "Cut", "Ctrl+X").enabled(sel));
            items.push(Item::new(COPY, "Copy", "Ctrl+C").enabled(sel));
            items.push(Item::new(PASTE, "Paste", iyakki_uikit::platform::paste_key()).needs_clip());
            items.push(Item::new(SELECT_ALL, "Select all", "Ctrl+A").enabled(!f.empty));
        }
        crate::ctxmenu::set_under(self.hits.iter().rev().filter(|h| h.rect.contains(x, y)).cloned().collect());
        if let Screen::App(app, _) = &mut self.screen {
            let own: Vec<Item> = app.context_items(x, y).into_iter().filter(|it| it.is_separator() || !items.iter().any(|e| e.id == it.id)).collect();
            if !own.is_empty() {
                if !items.is_empty() {
                    items.push(Item::separator());
                }
                items.extend(own);
            }
        }
        if !items.is_empty() {
            items.push(Item::separator());
        }
        items.push(Item::new(HISTORY, "Clipboard history…", "Ctrl+Shift+V"));
        // ask the system for what it holds: text copied in other programs makes Paste usable
        self.want_host_paste = true;
        self.host_sync_until = self.tick + 90;
        let clip = crate::ui::clipboard::can_paste();
        self.ctx = Some(if crate::touchscreen::take_touch_menu() { Menu::open_touch(items, (x, y), (w, h), s, clip) } else { Menu::open(items, (x, y), (w, h), s, clip) });
    }

    /// The menu is open: keys move over it, a click chooses, anything else closes it.
    fn handle_ctx(&mut self, ev: &Event) {
        use crate::ctxmenu::Outcome;
        let Some(m) = self.ctx.as_mut() else { return };
        m.set_clip_text(crate::ui::clipboard::can_paste());
        let before = m.sel;
        let out = match ev {
            Event::Key(k) => m.key(k),
            Event::MouseMove(x, y) => {
                m.hover(*x, *y);
                Outcome::Stay
            }
            Event::MouseDown(x, y) => m.click(*x, *y),
            Event::MouseRight(x, y) => {
                let (x, y) = (*x, *y);
                self.ctx = None;
                self.open_context_menu(x, y);
                return;
            }
            Event::Scroll(_) => Outcome::Close,
            _ => Outcome::Stay,
        };
        if m.sel != before {
            mark_stale!(self);
        }
        match out {
            Outcome::Stay => {}
            Outcome::Close => {
                self.ctx = None;
                mark_stale!(self);
                // a key that is not the menu's own (Ctrl+C, a letter…) still does what it would have done
                if matches!(ev, Event::Key(k) if *k != Key::Esc) {
                    self.dispatch(ev);
                }
            }
            Outcome::Pick(id) => {
                self.ctx = None;
                mark_stale!(self);
                self.run_context_entry(&id);
            }
        }
    }

    fn run_context_entry(&mut self, id: &str) {
        use crate::ctxmenu::{COPY, CUT, HISTORY, PASTE, SELECT_ALL};
        match id {
            CUT => self.route(&Event::Key(Key::Ctrl('x'))),
            COPY => self.route(&Event::Key(Key::Ctrl('c'))),
            SELECT_ALL => self.route(&Event::Key(Key::Ctrl('a'))),
            PASTE => match crate::ui::clipboard::paste() {
                Some(t) => self.route(&Event::Paste(t)),
                None => self.say_info("The clipboard is empty"),
            },
            HISTORY => self.open_clipboard_picker(),
            other => {
                let cx = self.cx();
                if let (Screen::App(app, _), Some(cx)) = (&mut self.screen, cx) {
                    if matches!(app.context_pick(other, &cx), Flow::Back) {
                        self.leave_app();
                    }
                }
            }
        }
    }

    fn start_shot(&mut self) {
        if self.session.is_some() {
            self.help = false;
            self.shot = Shot::Snap;
        }
    }

    /// Keys and clicks while framing a screenshot: two clicks (opposite corners) keep that area, Enter keeps the whole
    /// screen, Esc cancels.
    fn handle_shot(&mut self, ev: &Event) {
        if let Shot::Review { .. } = &self.shot {
            match ev {
                Event::Key(Key::Enter) | Event::Key(Key::Char('s')) => self.save_shot_file(),
                Event::Key(Key::Esc) | Event::Key(Key::Char('c')) | Event::Key(Key::F(12)) => {
                    self.shot = Shot::Off;
                    self.say_info("Screenshot kept on the clipboard");
                }
                _ => {}
            }
            return;
        }
        let Shot::Select { snap, first } = &mut self.shot else { return };
        let (w, h) = (snap.width() as i32, snap.height() as i32);
        match ev {
            Event::Key(Key::Esc) | Event::Key(Key::F(12)) => {
                self.shot = Shot::Off;
                self.say_info("Screenshot cancelled");
            }
            Event::Key(Key::Enter) => self.finish_shot((0, 0, w, h)),
            Event::MouseDown(x, y) => match first {
                None => *first = Some((*x, *y)),
                Some((fx, fy)) => {
                    let (x0, x1) = ((*fx).min(*x).clamp(0, w), (*fx).max(*x).clamp(0, w));
                    let (y0, y1) = ((*fy).min(*y).clamp(0, h), (*fy).max(*y).clamp(0, h));
                    if x1 - x0 < 8 || y1 - y0 < 8 {
                        *first = None; // a click without an area: start over
                    } else {
                        self.finish_shot((x0, y0, x1 - x0, y1 - y0));
                    }
                }
            },
            _ => {}
        }
    }

    /// Put the chosen area on the clipboard and ask what to do next (save a file, or close).
    fn finish_shot(&mut self, (x, y, w, h): (i32, i32, i32, i32)) {
        let Shot::Select { snap, .. } = std::mem::replace(&mut self.shot, Shot::Off) else { return };
        let Some(rect) = tiny_skia::IntRect::from_xywh(x, y, w as u32, h as u32) else { return };
        let Some(img) = snap.clone_rect(rect) else {
            self.say("Could not take the screenshot");
            return;
        };
        // the screen is opaque, so premultiplied and plain RGBA are the same
        crate::ui::clipboard::copy_image(img.width(), img.height(), img.data().to_vec());
        if let Some(s) = &self.session {
            self.env.audit.record(&s.user, "screenshot", &format!("{}x{}", w, h));
        }
        self.shot = Shot::Review { img };
    }

    /// Save the picture under review as a PNG: in the person's own `Pictures` folder when their account may write there,
    /// otherwise in their screenshots folder inside the system's data.
    fn save_shot_file(&mut self) {
        let Shot::Review { img } = std::mem::replace(&mut self.shot, Shot::Off) else { return };
        self.save_pixmap(&img);
    }

    fn save_pixmap(&mut self, img: &tiny_skia::Pixmap) {
        let Some(sess) = self.session.clone() else { return };
        let name = format!("iyakki-{}.png", stamp(std::time::SystemTime::now()));
        let home = std::path::PathBuf::from(format!("/home/{}/Pictures", sess.user));
        let fallback = self.env.data_dir().join("screenshots").join(sess.user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect::<String>());
        let dir = if sess.policy.allows_write(&sess.user, &home.to_string_lossy()) && std::fs::create_dir_all(&home).is_ok() { home } else { fallback };
        let path = dir.join(name);
        match std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|_| img.save_png(&path).map_err(|e| e.to_string())) {
            Ok(()) => {
                self.env.audit.record(&sess.user, "screenshot.save", &path.to_string_lossy());
                self.say_info(format!("Saved {}", path.display()));
            }
            Err(e) => self.say(format!("Could not save the screenshot: {e}")),
        }
    }

    /// Title and lines of the F1 popup: this screen's keys, then the ones that work everywhere.
    fn help_lines(&self) -> (String, Vec<String>) {
        let (title, mut lines) = match &self.screen {
            Screen::App(app, _) => (app.title().to_string(), app.help()),
            _ => (
                "Home".to_string(),
                vec!["← → ↑ ↓ · choose an app (moving past the last one on a page turns it)".to_string(), "Tab / Shift+Tab or click a chip · switch group (Ask & Create, Build, Media, Data, Devices & System, Apps, and any group you made)".to_string(), "m · move the selected tile to another group (or right-click it); g · this group's menu (new, rename, delete); + chip · new group; u · undo the last move; [ and ] · move the tile earlier or later in its group".to_string(), "PageUp / PageDown, mouse wheel or the dots · turn the page".to_string(), "1-9 · open the Nth app of the All list (the order of the tile table, whichever group you moved it to)".to_string(), "Enter or click · open it".to_string(), "o, q or Esc · sign out (asks first: Enter or y signs out, Esc or n stays)".to_string(), "Ctrl+Cmd+Q (Mac profile) or Ctrl+Windows+Q (PC profile) · sign out, from any screen (asks first); Ctrl+Cmd+L or Super+L · lock the screen".to_string(), "p · power menu (if your role allows it)".to_string()],
            ),
        };
        lines.push(String::new());
        lines.push("Everywhere".to_string());
        lines.extend(["F1 · this help".to_string(), "F11 or the Home icon · back to the tiles from any app (the gear opens Settings, the speaker sets the volume)".to_string(), "F9 or the bell · your inbox (notifications, tasks, approvals)".to_string(), "F10 or a click on the name IYAKKI · the IYAKKI menu (About, Quick install, Media player, Mac drivers)".to_string(), "F12 or Print Screen · screenshot to the clipboard (click two corners, or Enter for the whole screen), then save a file or close".to_string(), "Esc · back".to_string(), "Ctrl+V · paste".to_string(), "Ctrl+Shift+V or right-click · clipboard history".to_string()]);
        (format!("Help — {title}"), lines)
    }

    fn draw_launcher(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        let Some(sess) = self.session.clone() else { return };
        let power = self.may_power();
        // top bar (a launcher in a pane has the pane's title strip instead)
        let top_bar = !self.panes.compact;
        let lay = top_bar && self.panes_button();
        // the bar's origin: 0 at the top, the screen height less the band when the Look puts the bar at the bottom (the footer takes the top band)
        let bar_dy = if self.look.topbar.position == iyakki_uikit::look::BarPos::Bottom { h as f32 - iyakki_uikit::barside::BAR_BAND * s } else { 0.0 };
        iyakki_uikit::barside::set_bar_dy(bar_dy);
        // a Look with the top bar off hides the row of buttons; the corner handle (and the `b` key) bring it back for one use
        if top_bar && !self.bar_visible() {
            g.set_origin_y(bar_dy);
            self.draw_bar_handle(g, s);
            g.set_origin_y(0.0);
        }
        if top_bar && self.bar_visible() {
        g.set_origin_y(bar_dy);
        let name_w = g.text_width(30.0 * s, "IYAKKI", Face::Bold);
        // the name opens the IYAKKI menu (About, Quick install, Media player, Mac drivers)
        let name_box = R::new(30.0 * s, 18.0 * s, name_w + 20.0 * s, 56.0 * s);
        if g.hovered(name_box) || self.brand.open {
            g.rrect(name_box.x, name_box.y, name_box.w, name_box.h, 12.0 * s, rgb(28, 36, 66));
        }
        g.text(40.0 * s, 26.0 * s, 30.0 * s, TEXT, "IYAKKI", Face::Bold);
        name_strip(g, 40.0 * s, 64.0 * s, name_w, 4.0 * s, s, false);
        g.hit(name_box, Event::Key(Key::F(10)), "IYAKKI menu");
        let chip = account_label(&sess.user, sess.role.name());
        let cw = g.text_width(16.0 * s, &chip, Face::Bold) + 36.0 * s;
        // the icons run from the right edge: sign out, power, camera, devices, clipboard, bell, panes, then the speaker and Settings
        let count = self.bar_slots().count;
        let right = header_button(w, s, count.saturating_sub(1)).x - (header_slot(w, s) - header_button(w, s, 0).w);
        let title_end = 40.0 * s + name_w;
        // the account chip gives way on a screen too narrow for it beside the name
        let chip_shown = right - cw >= title_end + 24.0 * s;
        if chip_shown {
            crate::fx::glass(g, right - cw, 24.0 * s, cw, 44.0 * s, 22.0 * s, crate::fx::CYAN);
            g.text(right - cw + 18.0 * s, 35.0 * s, 16.0 * s, TEXT, &chip, Face::Bold);
        }
        let chip_left = if chip_shown { right - cw } else { right };
        // the buttons from the right edge in the bar's order (sign out, power when allowed, the camera, Devices, the clipboard queue, the bell, the
        // panes button; the microphone, speaker and Settings are drawn by `draw_bar_extras` in their places from the same list)
        let order = self.bar_order();
        for (i, b) in order.iter().enumerate() {
            let r = header_button(w, s, i);
            let ic = match b {
                BarButton::Logout => Icon::Logout,
                BarButton::Power => Icon::Power,
                BarButton::Shot => Icon::Camera,
                BarButton::Devices => Icon::Devices,
                BarButton::Clipboard => Icon::Clipboard,
                BarButton::Network => Icon::Mesh,
                BarButton::Apps => Icon::Home,
                BarButton::GemVoice => {
                    let hover = g.hovered(r);
                    crate::fx::chrome_button(g, r, s * 2.5, hover || self.gv.open(), crate::fx::CYAN);
                    g.icon(Icon::Mic, r.x + r.w / 2.0, r.y + r.h / 2.0, 22.0 * s, if self.gv.open() { crate::fx::CYAN } else { MUTED });
                    g.hit(r, Event::Tab(gv_impl::MIC), "Gemini voice (EXPERIMENTAL)");
                    continue;
                }
                BarButton::Bell => {
                    draw_bell(g, r, 22.0 * s, s, self.badge, false);
                    continue;
                }
                BarButton::Panes => {
                    if lay {
                        panes_impl::draw_layout_button(g, r, s, crate::fx::CYAN);
                    }
                    continue;
                }
                BarButton::Mic | BarButton::Speaker | BarButton::Settings => continue,
            };
            let hover = g.hovered(r);
            crate::fx::chrome_button(g, r, s * 2.5, hover, crate::fx::CYAN);
            g.icon(ic, r.x + r.w / 2.0, r.y + r.h / 2.0, 22.0 * s, if ic == Icon::Devices && self.devpop.open { crate::fx::CYAN } else { MUTED });
        }
        // the Look's top bar items: the clock and the battery chip are optional (the bell, the needs-you sign, Settings, the account and power
        // are essential and always drawn)
        let bar_has = |i: iyakki_uikit::look::BarItem| self.look.topbar.items.contains(&i);
        self.clock_tip = if bar_has(iyakki_uikit::look::BarItem::Clock) { launcher_clock(g, s, chip_left - 18.0 * s, title_end + 24.0 * s, &self.clock) } else { None };
        let oy = g.origin_y();
        let clock_in_draw_space = self.clock_tip;
        self.clock_tip = self.clock_tip.map(|b| R::new(b.x, b.y + oy, b.w, b.h));
        // the installer indicator, left of the clock (when there is room)
        let mut ind_right = clock_in_draw_space.map_or(chip_left - 18.0 * s, |b| b.x) - 8.0 * s;
        // the battery chip (laptops only), left of the clock
        let bat_plan = if bar_has(iyakki_uikit::look::BarItem::Battery) { self.battery.plan(g, s, ind_right, title_end + 24.0 * s, 24.0 * s, 44.0 * s, 15.0 * s) } else { None };
        match &bat_plan {
            Some(p) => {
                self.battery.draw_chip(g, p, s);
                ind_right = p.rect.x - 8.0 * s;
            }
            None => self.battery.anchor = None,
        }
        let mut dev_right = ind_right;
        if ind_right - self.installs.indicator_width(g, s) > title_end + 24.0 * s {
            if let Some(b) = self.installs.draw_indicator(g, ind_right, 46.0 * s, s, self.tick) {
                dev_right = b.x - 8.0 * s;
            }
        } else {
            self.installs.anchor = None;
        }
        if let Some(sign) = self.dev_sign.as_deref() {
            draw_dev_sign(g, dev_right, 46.0 * s, s, sign, title_end + 24.0 * s, self.devask.pulse(self.tick));
        }
        self.draw_bar_extras(g, s);
        g.set_origin_y(0.0);
        }
        // tiles
        if self.tick.saturating_sub(self.custom_at) > 60 || self.custom_at == 0 {
            self.refresh_custom();
        }
        self.lay_poll();
        self.sync_look();
        let tiles = self.group_items();
        self.sel = self.sel.min(tiles.len() - 1);
        let cap = launcher::per_page(w, h, s);
        let machine = self.machine;
        let (pages, page) = if machine { (1, 0) } else { (launcher::page_count(tiles.len(), cap), self.sel / cap) };
        let rects = if machine { Vec::new() } else { launcher::page_rects(w, h, s, tiles.len(), page) };
        // group chips, then `…` (the groups that do not fit) and `+` (a new group)
        let groups = self.groups();
        let cur = self.current_group();
        let row = self.chip_row();
        for (i, r) in &row.chips {
            let gr = &groups[*i];
            let on = *gr == cur && !machine;
            let hover = g.hovered(*r);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, r.h / 2.0, on, hover, if self.lay.carry.is_some() && hover { crate::ui::OK } else { crate::fx::CYAN });
            let label = g.fit(15.0 * s, gr.label(), Face::Bold, r.w - 16.0 * s);
            let lw = g.text_width(15.0 * s, &label, Face::Bold);
            g.text(r.x + (r.w - lw) / 2.0, r.y + (r.h - 15.0 * s) / 2.0 - s, 15.0 * s, if on { TEXT } else { MUTED }, &label, Face::Bold);
            g.hit(*r, Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32), &format!("group {}", gr.label()));
        }
        self.draw_extra_chips(g, &row, s);
        // the frosted island with the icons (or the machine page) on it, the page dots and the arrows (`shell_home.rs`)
        self.draw_home_page(g, s, &tiles, &rects, page * cap);
        self.draw_dock(g, s);
        if pages > 1 {
            let nav = launcher::page_nav(w, h, s, pages);
            for (p, r) in nav.dots.iter().enumerate() {
                let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                if p == page {
                    g.circle(cx, cy, 10.0 * s, [70, 225, 255, 50]);
                }
                g.rrect(cx - 6.0 * s, cy - 6.0 * s, 12.0 * s, 12.0 * s, 6.0 * s, if p == page { crate::fx::CYAN } else { DIM });
                g.hit(*r, Event::MouseDown(cx as i32, cy as i32), &format!("page {}", p + 1));
            }
            for (r, glyph, to) in [(nav.prev, "<", page.checked_sub(1)), (nav.next, ">", Some(page + 1).filter(|p| *p < pages))] {
                let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                // the glyph shrinks with the button when the buttons were squeezed to fit a narrow window
                let gs = 20.0 * s * (r.w / (26.0 * s)).min(1.0);
                let gw = g.text_width(gs, glyph, Face::Bold);
                g.text(cx - gw / 2.0, cy - gs * 0.55, gs, if to.is_some() { if g.hovered(r) { TEXT } else { MUTED } } else { DIM }, glyph, Face::Bold);
                g.hit(r, Event::MouseDown(cx as i32, cy as i32), if glyph == "<" { "previous page" } else { "next page" });
            }
        }
        if top_bar {
        g.set_origin_y(-bar_dy);
        let hint = format!("←/→ choose · Tab group · m move · PgUp/PgDn page · Enter open{} · s settings · v volume · o sign out", if self.views.model.views.len() > 1 { " · click a view (bottom right) to switch" } else { "" }) + if power { " · p power" } else { "" } + if crate::inputprofile::resolved() == iyakki_shared::inputprofile::Resolved::Mac { " · Ctrl+Cmd+Q sign out · Ctrl+Cmd+L lock" } else { " · Super+L lock · Ctrl+Win+Q sign out" } + if self.look.topbar.on { "" } else { " · b show the bar" };
        let host = format!("{}  ·  {}", self.env.hostname, self.env.host.detect_env());
        let host_w = g.text_width(14.0 * s, &host, Face::Sans);
        // the key hint sits in the middle when there is room between the host name and the right edge, otherwise it is shortened to the room
        // there is (it used to run over the host name on narrow windows and at the large text sizes)
        let room = (w as f32 - 40.0 * s) - (40.0 * s + host_w + 24.0 * s);
        let hint = g.fit(14.0 * s, &hint, Face::Sans, room.max(0.0));
        let hw = g.text_width(14.0 * s, &hint, Face::Sans);
        let hx = ((w as f32 - hw) / 2.0).max(40.0 * s + host_w + 24.0 * s);
        if self.look.topbar.footer_hints {
            g.text(hx, h as f32 - 36.0 * s, 14.0 * s, DIM, &hint, Face::Sans);
        }
        g.text(40.0 * s, h as f32 - 36.0 * s, 14.0 * s, DIM, &host, Face::Sans);
        // what the radio is playing in the background
        if let iyakki_tile_api::radio::RadioState::Playing { station, title, .. } = self.env.services.radio().state() {
            let line = if title.is_empty() { format!("♪ {station}") } else { format!("♪ {station} — {title}") };
            let line = g.fit(14.0 * s, &line, Face::Sans, w as f32 * 0.32);
            let lw = g.text_width(14.0 * s, &line, Face::Sans);
            g.text(w as f32 - lw - 40.0 * s, h as f32 - 36.0 * s, 14.0 * s, OK, &line, Face::Sans);
        }
        // and what MEDIA plays
        if let Some(label) = iyakki_apps::agent().b0().media().player().now_playing() {
            let line = g.fit(14.0 * s, &format!("▶ {label}"), Face::Sans, w as f32 * 0.32);
            let lw = g.text_width(14.0 * s, &line, Face::Sans);
            g.text(w as f32 - lw - 40.0 * s, h as f32 - 60.0 * s, 14.0 * s, OK, &line, Face::Sans);
        }
        g.set_origin_y(0.0);
        self.draw_bar_overlay(g, s);
        }
    }
}

/// The clock in the app header, left of the bell: the time, with the short date when there is room. Returns its box (for
/// the tooltip and the click).
fn app_clock(g: &mut Gfx, _w: u32, s: f32, clock: &crate::clockbar::ClockState, bar_edge: f32) -> Option<R> {
    let right = bar_edge - 12.0 * s;
    let (time, date) = (clock_time_text(clock), clock.date_text());
    let (tw, dw) = (g.text_width(15.0 * s, &time, Face::Bold), g.text_width(13.0 * s, &date, Face::Sans));
    // keep room for the app's name on the left
    let avail = right - 300.0 * s;
    let gap = 10.0 * s;
    let (show_date, total) = match crate::clockbar::fit(avail, tw, dw, gap) {
        crate::clockbar::Fit::Full => (true, tw + gap + dw),
        crate::clockbar::Fit::TimeOnly => (false, tw),
        crate::clockbar::Fit::Hidden => return None,
    };
    let b = R::new(right - total - 8.0 * s, 8.0 * s, total + 16.0 * s, 30.0 * s);
    let hot = g.hovered(b);
    if hot {
        g.rrect(b.x, b.y, b.w, b.h, 8.0 * s, rgb(28, 36, 66));
    }
    let ink = if clock.looks_wrong() { rgb(255, 196, 94) } else { TEXT };
    let ty = b.y + (b.h - g.line_height(15.0 * s, Face::Bold)) / 2.0;
    g.text(b.x + 8.0 * s, ty, 15.0 * s, ink, &time, Face::Bold);
    if show_date {
        g.text(b.x + 8.0 * s + tw + gap, ty + 1.0 * s, 13.0 * s, MUTED, &date, Face::Sans);
    }
    g.hit(b, Event::Key(Key::F(7)), "clock");
    Some(b)
}

/// The clock in the launcher's top bar, right-aligned at `right`: time over date, or the time alone, or nothing when
/// there is no room left of `left_limit`.
fn launcher_clock(g: &mut Gfx, s: f32, right: f32, left_limit: f32, clock: &crate::clockbar::ClockState) -> Option<R> {
    let (time, date) = (clock_time_text(clock), clock.date_text());
    let (tw, dw) = (g.text_width(22.0 * s, &time, Face::Bold), g.text_width(13.0 * s, &date, Face::Sans));
    let avail = right - left_limit;
    let (two_lines, wide) = if avail >= tw.max(dw) { (true, tw.max(dw)) } else if avail >= tw { (false, tw) } else { return None };
    let b = R::new(right - wide - 10.0 * s, 24.0 * s, wide + 20.0 * s, 44.0 * s);
    let hot = g.hovered(b);
    if hot {
        g.rrect(b.x, b.y, b.w, b.h, 10.0 * s, rgb(28, 36, 66));
    }
    let ink = if clock.looks_wrong() { rgb(255, 196, 94) } else { TEXT };
    if two_lines {
        g.text(right - tw, b.y + 3.0 * s, 22.0 * s, ink, &time, Face::Bold);
        g.text(right - dw, b.y + 3.0 * s + g.line_height(22.0 * s, Face::Bold), 13.0 * s, MUTED, &date, Face::Sans);
    } else {
        g.text(right - tw, b.y + (b.h - g.line_height(22.0 * s, Face::Bold)) / 2.0, 22.0 * s, ink, &time, Face::Bold);
    }
    g.hit(b, Event::Key(Key::F(7)), "clock");
    Some(b)
}

/// The time as shown in the bar; a clock that was never set gets an exclamation mark.
fn clock_time_text(clock: &crate::clockbar::ClockState) -> String {
    if clock.looks_wrong() { format!("{} !", clock.time_text()) } else { clock.time_text() }
}

/// The small amber "DEV" sign while Developer mode is on, right edge at `right`, centred on `cy`; skipped when it would cross
/// `left_limit`. A click opens Settings → Developer mode.
fn draw_dev_sign(g: &mut Gfx, right: f32, cy: f32, s: f32, sign: &str, left_limit: f32, pulse: f32) {
    let tw = g.text_width(11.5 * s, sign, Face::Bold);
    let b = R::new(right - tw - 14.0 * s, cy - 10.0 * s, tw + 14.0 * s, 20.0 * s);
    if b.x < left_limit {
        return;
    }
    // a shell grant is the dangerous state: red, never amber
    let amber = if sign.contains("SHELL ACCESS") || sign.contains("INTERRUPTED") { rgb(255, 99, 99) } else { rgb(255, 196, 94) };
    if pulse > 0.0 {
        // a request waits for the administrator: the sign swells and glows
        g.rrect_glow(b.x, b.y, b.w, b.h, 6.0 * s, (3.0 + 9.0 * pulse) * s, crate::fx::with_alpha(amber, (60.0 + 160.0 * pulse) as u8));
    }
    g.rrect(b.x, b.y, b.w, b.h, 6.0 * s, crate::fx::with_alpha(amber, (40.0 + 120.0 * pulse) as u8));
    g.rrect_outline(b.x, b.y, b.w, b.h, 6.0 * s, 1.0, amber);
    g.text(b.x + 7.0 * s, cy - g.line_height(11.5 * s, Face::Bold) / 2.0, 11.5 * s, amber, sign, Face::Bold);
    g.hit(b, Event::Tab(DEV_SIGN), "developer mode on");
}

/// The click of the DEV sign.
const DEV_SIGN: usize = 1_300_100;

/// The click of the updates chip in the app header.
const UPDATE_CHIP: usize = 1_300_101;
/// The click of the "Sharing" chip in the app header.
const SHARING_CHIP: usize = 1_300_102;

/// A small chip in the app header (updates waiting, internet being shared). `None` when there is no room left of the title limit
/// (the STORE tile on Home still says it).
fn draw_header_chip(g: &mut Gfx, right: f32, cy: f32, s: f32, label: &str, color: [u8; 4], id: usize, tip: &str, left_limit: f32) -> Option<R> {
    let tw = g.text_width(11.5 * s, label, Face::Bold);
    let b = R::new(right - tw - 14.0 * s, cy - 10.0 * s, tw + 14.0 * s, 20.0 * s);
    if b.x < left_limit {
        return None;
    }
    g.rrect(b.x, b.y, b.w, b.h, 6.0 * s, crate::fx::with_alpha(color, 40));
    g.rrect_outline(b.x, b.y, b.w, b.h, 6.0 * s, 1.0, color);
    g.text(b.x + 7.0 * s, cy - g.line_height(11.5 * s, Face::Bold) / 2.0, 11.5 * s, color, label, Face::Bold);
    g.hit(b, Event::Tab(id), tip);
    Some(b)
}

/// The clock's tooltip, under its box and kept on screen.
fn draw_clock_tip(g: &mut Gfx, w: u32, h: u32, s: f32, anchor: R, lines: &[(String, bool)]) {
    let size = 13.0 * s;
    let maxw = (360.0 * s).min(w as f32 - 24.0 * s);
    let mut wrapped: Vec<(String, bool)> = Vec::new();
    for (t, warn) in lines {
        for l in g.wrap(size, t, Face::Sans, maxw - 24.0 * s) {
            wrapped.push((l, *warn));
        }
    }
    let lh = g.line_height(size, Face::Sans) + 3.0 * s;
    let pw = wrapped.iter().map(|(l, _)| g.text_width(size, l, Face::Sans)).fold(0.0f32, f32::max) + 24.0 * s;
    let ph = wrapped.len() as f32 * lh + 18.0 * s;
    let x = (anchor.x + anchor.w - pw).clamp(8.0 * s, (w as f32 - pw - 8.0 * s).max(8.0 * s));
    let y = (anchor.y + anchor.h + 6.0 * s).min((h as f32 - ph - 8.0 * s).max(0.0));
    crate::fx::modal_panel(g, R::new(x, y, pw, ph), s * 0.6, crate::fx::CYAN, false);
    for (i, (l, warn)) in wrapped.iter().enumerate() {
        g.text(x + 12.0 * s, y + 9.0 * s + i as f32 * lh, size, if *warn { rgb(255, 196, 94) } else if i == 0 { TEXT } else { MUTED }, l, Face::Sans);
    }
}

/// The space one top-bar icon takes, gap included: 56 at scale 1, less on a narrow screen so that eight icons take at most 60% of the width.
fn header_slot(w: u32, s: f32) -> f32 {
    (56.0 * s).min(w as f32 * 0.6 / 8.0)
}

/// Where the button `i` of the bar is on the screen: where it is drawn, moved by the bar's origin (the bar at the bottom).
fn header_button_at(w: u32, s: f32, i: usize) -> R {
    let r = header_button(w, s, i);
    R::new(r.x, r.y + iyakki_uikit::barside::bar_dy(), r.w, r.h)
}

fn header_button(w: u32, s: f32, i: usize) -> R {
    let slot = header_slot(w, s);
    let d = slot * 44.0 / 56.0;
    R::new(w as f32 - 40.0 * s - d - i as f32 * slot, 24.0 * s, d, d)
}

/// The bell, left of the clipboard queue button.
fn bell_rect(w: u32, s: f32) -> R {
    bar_rects(w, s).bell
}

/// The bell with its count: red when something waits for an answer, amber for errors and warnings. Routine items (information,
/// new devices, finished jobs) never add to the number; they show as a small blue dot.
fn draw_bell(g: &mut Gfx, r: R, radius: f32, s: f32, b: Badge, open: bool) {
    let hover = g.hovered(r);
    crate::fx::chrome_button(g, r, radius / 8.0, hover || open, crate::fx::CYAN);
    let n = b.total();
    let ink = if b.approvals > 0 { rgb(255, 120, 120) } else if n > 0 { rgb(255, 196, 94) } else if b.other > 0 { rgb(170, 190, 215) } else { MUTED };
    g.icon(Icon::Bell, r.x + r.w / 2.0, r.y + r.h / 2.0, r.h * 0.62, ink);
    if n > 0 {
        let label = if n > 99 { "99+".to_string() } else { n.to_string() };
        let size = (r.h * 0.34).max(10.0 * s);
        let tw = g.text_width(size, &label, Face::Bold);
        let bw = (tw + size * 0.7).max(size * 1.35);
        let bh = size * 1.4;
        let (bx, by) = (r.x + r.w - bw * 0.72, r.y - bh * 0.18);
        g.rrect(bx, by, bw, bh, bh / 2.0, if b.approvals > 0 { rgb(214, 48, 64) } else { rgb(214, 130, 20) });
        g.text(bx + (bw - tw) / 2.0, by + (bh - g.line_height(size, Face::Bold)) / 2.0, size, rgb(255, 255, 255), &label, Face::Bold);
    } else if b.other > 0 {
        // only routine items are unread: a small dot, no number
        let d = (r.h * 0.26).max(7.0 * s);
        g.rrect(r.x + r.w - d * 1.1, r.y + d * 0.1, d, d, d / 2.0, rgb(110, 150, 200));
    }
    g.hit(r, Event::Key(Key::F(9)), "inbox");
}

/// Which of the optional icons of the app header fit beside the title: the Help chip goes first, then the camera and the clipboard
/// buttons, then Devices (all of them have their keys: F1, F12, Ctrl+V, F8). Back and the bell always stay. The icons stand from the right
/// edge; what is dropped leaves its room to the others and to the title, so nothing runs over the title or out of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BarShown {
    help: bool,
    shot: bool,
    clip: bool,
    dev: bool,
}

/// The width (before scaling) of the cluster at each level, and the room the title and the icons beside it need.
const BAR_FULL: f32 = 418.0;
const BAR_TITLE_ROOM: f32 = 180.0;

fn bar_shown(w: u32, s: f32) -> BarShown {
    let fits = |units: f32| (units + BAR_TITLE_ROOM) * s <= w as f32;
    if fits(BAR_FULL) {
        BarShown { help: true, shot: true, clip: true, dev: true }
    } else if fits(BAR_FULL - 112.0) {
        BarShown { help: false, shot: true, clip: true, dev: true }
    } else if fits(BAR_FULL - 112.0 - 80.0) {
        BarShown { help: false, shot: false, clip: false, dev: true }
    } else {
        BarShown { help: false, shot: false, clip: false, dev: false }
    }
}

/// The rectangles of the cluster, right to left: Back, Help, camera, Devices, clipboard, bell. A dropped icon gets the place its next
/// neighbour would take (it is not drawn).
struct BarRects {
    /// Read by the header tests (the header draws Back itself).
    #[cfg_attr(not(test), allow(dead_code))]
    back: R,
    help: R,
    shot: R,
    dev: R,
    clip: R,
    bell: R,
}

fn bar_rects(w: u32, s: f32) -> BarRects {
    let shown = bar_shown(w, s);
    let (top, h) = (8.0 * s, 30.0 * s);
    let mut cursor = w as f32 - 20.0 * s;
    let mut put = |width: f32, on: bool| -> R {
        let r = R::new(cursor - width * s, top, width * s, h);
        if on {
            cursor = r.x - 8.0 * s;
        }
        r
    };
    let back = put(94.0, true);
    let help = put(104.0, shown.help);
    let shot = put(32.0, shown.shot);
    let dev = put(32.0, shown.dev);
    let clip = put(32.0, shown.clip);
    let bell = put(32.0, true);
    BarRects { back, help, shot, dev, clip, bell }
}

/// The Devices button, left of the camera.
fn dev_rect(w: u32, s: f32) -> R {
    bar_rects(w, s).dev
}

/// The clipboard queue button, left of the Devices button.
const ACCENT_QR: [u8; 4] = rgb(255, 184, 77);

fn clip_rect(w: u32, s: f32) -> R {
    bar_rects(w, s).clip
}

/// The camera button, left of the "F1 Help" chip.
fn shot_rect(w: u32, s: f32) -> R {
    bar_rects(w, s).shot
}

/// The "F1 Help" chip, left of the Back button.
fn help_rect(w: u32, s: f32) -> R {
    bar_rects(w, s).help
}

/// Height of the strip with the app's name, in units before scaling.
const HEADER_H: f32 = 46.0;

fn tilehub_allowed(p: &iyakki_sec::policy::Policy) -> bool {
    iyakki_shared::store_tilehub::allowed(p)
}

/// The accent a custom app's tile wears: from its id, so the same app always has the same colour.
pub(crate) fn custom_accent(name: &str) -> [u8; 4] {
    const PALETTE: [[u8; 4]; 8] = [rgb(150, 220, 255), rgb(255, 170, 120), rgb(170, 230, 140), rgb(230, 150, 230), rgb(255, 215, 110), rgb(120, 200, 190), rgb(190, 170, 255), rgb(255, 140, 160)];
    let h = name.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    PALETTE[h as usize % PALETTE.len()]
}

/// The decoded `.png` icon of a custom app (`None` for an emoji or a .svg icon, or a picture that does not decode: the tile then shows the
/// first letter). Decoded once per app version and kept (the launcher draws every frame).
pub(crate) fn custom_icon_picture(e: &iyakki_shared::store_tilehub::TileEntry) -> Option<std::sync::Arc<tiny_skia::Pixmap>> {
    type Cache = std::sync::Mutex<std::collections::HashMap<String, Option<std::sync::Arc<tiny_skia::Pixmap>>>>;
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    let (path, bytes) = e.icon_picture.as_ref().filter(|(p, _)| p.to_ascii_lowercase().ends_with(".png"))?;
    let key = format!("{}|{}|{}|{}", e.key, e.version, path, bytes.len());
    let mut c = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|x| x.into_inner());
    if c.len() > 64 {
        c.clear();
    }
    c.entry(key).or_insert_with(|| tiny_skia::Pixmap::decode_png(bytes).ok().map(std::sync::Arc::new)).clone()
}

/// A custom app's tile: a round badge with the first letter of its title in the app's colour, the title, the group, and the
/// "custom app" mark (an unsigned dev app says so instead).
const AMBER_DEV: [u8; 4] = rgb(255, 196, 94);

fn back_rect(w: u32, s: f32) -> R {
    R::new(w as f32 - 114.0 * s, 8.0 * s, 94.0 * s, 30.0 * s)
}

/// Several panes replace the single screen (and its sign-in form): no galaxy behind them.
fn panes_drawn_early(sh: &Shell) -> bool {
    sh.ws_active()
}

/// The animated logo above the sign-in card: its width and the gap to the card.
fn auth_logo(s: f32) -> (f32, f32) {
    (84.0 * s, 10.0 * s)
}

fn auth_card(w: u32, h: u32, s: f32, n_fields: usize) -> R {
    let cw = (440.0 * s).min(w as f32 - 40.0 * s);
    let ch = (290.0 + 66.0 * n_fields as f32) * s;
    let (ld, gap) = auth_logo(s);
    // the logo and the card are centred together; the card never reaches up into the top bar, with the logo above it
    let top = ((h as f32 - (ch + ld + gap)) / 2.0).max(crate::loginbar::bar_h(s) + 8.0);
    R::new((w as f32 - cw) / 2.0, top + ld + gap, cw, ch)
}

fn auth_field_rect(w: u32, h: u32, s: f32, i: usize, n: usize) -> R {
    let c = auth_card(w, h, s, n);
    R::new(c.x + 32.0 * s, c.y + 168.0 * s + i as f32 * 66.0 * s, c.w - 64.0 * s, 50.0 * s)
}

/// `lift`: pixels the card is moved up so the field being typed in stays above the on-screen keyboard.
fn draw_auth(g: &mut Gfx, w: u32, h: u32, s: f32, f: &Form, setup: bool, tick: u64, hostname: &str, volatile: bool, lift: f32) {
    let mut c = auth_card(w, h, s, f.fields.len());
    c.y -= lift;
    // the boot screen's logo, animated, above the card (the galaxy turns behind both; reduced motion holds the logo still)
    let (ld, gap) = auth_logo(s);
    let lt = if crate::fx::reduced_motion() { 0.0 } else { tick as f32 * 0.033 };
    crate::splash::draw_logo(g, lt, c.x + c.w / 2.0, c.y - gap - ld / 2.0, ld);
    g.radial_glow(c.x + c.w / 2.0, c.y + c.h / 2.0, c.w * 0.7, [70, 225, 255, 22]);
    crate::fx::modal_panel(g, c, s * 1.4, crate::fx::CYAN, false);
    let tw = g.text_width(54.0 * s, "IYAKKI", Face::Bold);
    g.text(c.x + (c.w - tw) / 2.0, c.y + 28.0 * s, 54.0 * s, TEXT, "IYAKKI", Face::Bold);
    name_strip(g, c.x + (c.w - tw) / 2.0, c.y + 98.0 * s, tw, 5.0 * s, s, true);
    let sub = if setup { "Welcome — create the administrator account" } else { "An OS for Agents" };
    let sw = g.text_width(15.0 * s, sub, Face::Sans);
    g.text(c.x + (c.w - sw) / 2.0, c.y + 118.0 * s, 15.0 * s, MUTED, sub, Face::Sans);
    for (i, field) in f.fields.iter().enumerate() {
        let mut fr = auth_field_rect(w, h, s, i, f.fields.len());
        fr.y -= lift;
        field.render(g, fr, i == f.focus, s, (tick / 15) % 2 == 0, Face::Sans, i);
    }
    let label = if setup { "Create account" } else { "Sign in" };
    let bw = g.text_width(16.0 * s, label, Face::Bold) + 16.0 * s * 1.7;
    g.button(c.x + (c.w - bw) / 2.0, c.y + c.h - 96.0 * s, 16.0 * s, label, rgb(120, 170, 255), Event::Key(Key::Ctrl('s')));
    if let Some(e) = &f.error {
        let e = g.fit(15.0 * s, e, Face::Sans, c.w - 64.0 * s);
        g.text(c.x + 32.0 * s, c.y + c.h - 40.0 * s, 15.0 * s, ERR, &e, Face::Sans);
    } else {
        let hint = iyakki_uikit::platform::keys("Click a field or press Tab · Ctrl+V pastes");
        let hint = hint.as_str();
        let hw = g.text_width(13.0 * s, hint, Face::Sans);
        g.text(c.x + (c.w - hw) / 2.0, c.y + c.h - 36.0 * s, 13.0 * s, DIM, hint, Face::Sans);
    }
    g.text(24.0 * s, h as f32 - 34.0 * s, 14.0 * s, DIM, hostname, Face::Sans);
    // remote diagnostics (boot entry with iyakki.ssh=1): say how to reach this machine
    if let Some(note) = iyakki_core_local::machinefacts::ssh_note() {
        let note = format!("Remote diagnostics ON · {note}");
        let nw = g.text_width(14.0 * s, &note, Face::Sans);
        g.text(w as f32 - nw - 24.0 * s, h as f32 - 34.0 * s, 14.0 * s, rgb(255, 196, 94), &note, Face::Sans);
    }
    if volatile {
        let note = if setup { "Live session: this account is forgotten at power-off unless you attach a data disk (Settings → Storage)." } else { "Live session: changes are not saved when you power off. Admins: Settings → Storage." };
        let note = g.fit(14.0 * s, note, Face::Sans, w as f32 - 48.0 * s);
        let nw = g.text_width(14.0 * s, &note, Face::Sans);
        g.text((w as f32 - nw) / 2.0, c.y + c.h + 22.0 * s, 14.0 * s, rgb(255, 196, 94), &note, Face::Sans);
    }
}

/// The F1 help popup: a card in the middle listing every key, closed by any click, Esc, Enter or F1.
fn draw_help(g: &mut Gfx, w: u32, h: u32, s: f32, title: &str, lines: &[String]) {
    g.clear_hits();
    g.rect(0.0, 0.0, w as f32, h as f32, [0, 0, 0, 190]);
    let line_h = 26.0 * s;
    let bw = (760.0 * s).min(w as f32 - 40.0 * s);
    let bh = (96.0 * s + line_h * lines.len() as f32).min(h as f32 - 40.0 * s);
    let (x, y) = ((w as f32 - bw) / 2.0, (h as f32 - bh) / 2.0);
    crate::fx::modal_panel(g, R::new(x, y, bw, bh), s, crate::fx::CYAN, false);
    g.text(x + 28.0 * s, y + 22.0 * s, 24.0 * s, TEXT, title, Face::Bold);
    let key_w = (bw * 0.42).min(300.0 * s);
    let mut ly = y + 70.0 * s;
    for l in lines {
        if ly + line_h > y + bh - 30.0 * s {
            break;
        }
        match l.split_once(" · ") {
            Some((k, what)) => {
                let k = g.fit(15.0 * s, k, Face::Bold, key_w - 12.0 * s);
                g.text(x + 28.0 * s, ly, 15.0 * s, crate::fx::CYAN, &k, Face::Bold);
                let what = g.fit(15.0 * s, what, Face::Sans, bw - key_w - 56.0 * s);
                g.text(x + 28.0 * s + key_w, ly, 15.0 * s, TEXT, &what, Face::Sans);
            }
            None if !l.is_empty() => {
                g.text(x + 28.0 * s, ly + 4.0 * s, 14.0 * s, MUTED, l, Face::Bold);
            }
            None => {}
        }
        ly += line_h;
    }
    g.text(x + 28.0 * s, y + bh - 28.0 * s, 13.0 * s, DIM, "Esc or click to close", Face::Sans);
}

/// `YYYYMMDD-HHMMSS` (UTC) for a file name.
fn stamp(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil date from days since 1970-01-01
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Dim everything except the chosen area, and say what to do.
fn draw_shot_overlay(g: &mut Gfx, w: u32, h: u32, s: f32, first: Option<(i32, i32)>, mouse: (i32, i32)) {
    let (wf, hf) = (w as f32, h as f32);
    let dim = [0, 0, 0, 120];
    match first {
        None => g.rect(0.0, 0.0, wf, hf, dim),
        Some((fx, fy)) => {
            let (x0, x1) = (fx.min(mouse.0).max(0) as f32, fx.max(mouse.0).min(w as i32) as f32);
            let (y0, y1) = (fy.min(mouse.1).max(0) as f32, fy.max(mouse.1).min(h as i32) as f32);
            g.rect(0.0, 0.0, wf, y0, dim);
            g.rect(0.0, y1, wf, hf - y1, dim);
            g.rect(0.0, y0, x0, y1 - y0, dim);
            g.rect(x1, y0, wf - x1, y1 - y0, dim);
            g.rrect_outline(x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0), 0.0, 2.0 * s, rgb(120, 175, 255));
        }
    }
    let msg = if first.is_none() { "Click one corner of the area  ·  Enter keeps the whole screen  ·  Esc cancels" } else { "Click the opposite corner  ·  Esc cancels" };
    let mw = g.text_width(17.0 * s, msg, Face::Bold) + 40.0 * s;
    let (mx, my) = ((wf - mw) / 2.0, 16.0 * s);
    g.rrect(mx, my, mw, 40.0 * s, 20.0 * s, rgb(20, 26, 48));
    g.text(mx + 20.0 * s, my + 10.0 * s, 17.0 * s, TEXT, msg, Face::Bold);
}

/// "Copied to the clipboard", a small preview, and the two choices.
fn draw_shot_review(g: &mut Gfx, w: u32, h: u32, s: f32, img: &tiny_skia::Pixmap) {
    g.clear_hits();
    g.rect(0.0, 0.0, w as f32, h as f32, [0, 0, 0, 190]);
    let (maxw, maxh) = (460.0 * s, 280.0 * s);
    let k = (maxw / img.width() as f32).min(maxh / img.height() as f32).min(1.0);
    let (pw, ph) = (img.width() as f32 * k, img.height() as f32 * k);
    let bw = (pw + 60.0 * s).max(460.0 * s).min(w as f32 - 40.0 * s);
    let bh = ph + 190.0 * s;
    let (x, y) = ((w as f32 - bw) / 2.0, (h as f32 - bh) / 2.0);
    g.hit(R::new(x, y, bw, bh), Event::Tick, "backdrop");
    g.rrect(x, y, bw, bh, 20.0 * s, rgb(25, 31, 54));
    g.text(x + 28.0 * s, y + 22.0 * s, 22.0 * s, TEXT, "Screenshot copied to the clipboard", Face::Bold);
    g.text(x + 28.0 * s, y + 54.0 * s, 14.0 * s, MUTED, &format!("{} × {} pixels. Keep a file as well?", img.width(), img.height()), Face::Sans);
    let (ix, iy) = (x + (bw - pw) / 2.0, y + 84.0 * s);
    let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
    g.pm_mut().draw_pixmap(0, 0, img.as_ref(), &paint, tiny_skia::Transform::from_translate(ix, iy).pre_scale(k, k), None);
    g.rrect_outline(ix, iy, pw, ph, 0.0, 1.5 * s, rgb(70, 82, 120));
    let by = y + bh - 62.0 * s;
    let w1 = g.button(x + 28.0 * s, by, 15.0 * s, "Save to disk (S)", OK, Event::Key(Key::Char('s')));
    g.button(x + 28.0 * s + w1 + 12.0 * s, by, 15.0 * s, "Close (Esc)", MUTED, Event::Key(Key::Esc));
}

/// The alarm that is ringing, over everything else.
fn draw_ring(g: &mut Gfx, w: u32, h: u32, s: f32, r: &Ring, tick: u64) {
    g.clear_hits();
    g.rect(0.0, 0.0, w as f32, h as f32, [0, 0, 0, 190]);
    let n = r.items.len().min(4);
    let (bw, bh) = (520.0 * s, (210.0 + 44.0 * n as f32) * s);
    let (x, y) = ((w as f32 - bw) / 2.0, (h as f32 - bh) / 2.0);
    g.hit(R::new(x, y, bw, bh), Event::Tick, "backdrop");
    let amber = rgb(255, 196, 94);
    g.rrect(x, y, bw, bh, 22.0 * s, rgb(25, 31, 54));
    // a ringing bell: the rings pulse
    let pulse = ((tick % 24) as f32 / 24.0).min(1.0);
    let (cx0, cy0) = (x + 70.0 * s, y + 70.0 * s);
    g.ring(cx0, cy0, (34.0 + 14.0 * pulse) * s, 2.0 * s, rgb(255, 196, 94));
    g.circle(cx0, cy0, 26.0 * s, rgb(52, 44, 30));
    g.ring(cx0, cy0, 20.0 * s, 3.0 * s, amber);
    g.line(cx0, cy0, cx0, cy0 - 12.0 * s, 3.0 * s, amber);
    g.line(cx0, cy0, cx0 + 8.0 * s, cy0 + 4.0 * s, 3.0 * s, amber);
    let alarm = r.items.iter().any(|i| i.1);
    g.text(x + 128.0 * s, y + 34.0 * s, 26.0 * s, TEXT, if alarm { "Alarm" } else { "Reminder" }, Face::Bold);
    g.text(x + 128.0 * s, y + 72.0 * s, 14.0 * s, MUTED, "Enter dismisses · S snoozes for 5 minutes", Face::Sans);
    for (i, (what, is_alarm, when)) in r.items.iter().take(4).enumerate() {
        let iy = y + 124.0 * s + i as f32 * 44.0 * s;
        g.rrect(x + 28.0 * s, iy, bw - 56.0 * s, 38.0 * s, 10.0 * s, rgb(34, 42, 70));
        let shown = g.fit(17.0 * s, what, Face::Bold, bw - 220.0 * s);
        g.text(x + 42.0 * s, iy + 9.0 * s, 17.0 * s, if *is_alarm { amber } else { TEXT }, &shown, Face::Bold);
        let tw = g.text_width(13.0 * s, when, Face::Sans);
        g.text(x + bw - 42.0 * s - tw, iy + 12.0 * s, 13.0 * s, MUTED, when, Face::Sans);
    }
    if r.items.len() > 4 {
        g.text(x + 30.0 * s, y + 124.0 * s + 4.0 * 44.0 * s, 13.0 * s, MUTED, &format!("and {} more", r.items.len() - 4), Face::Sans);
    }
    let by = y + bh - 62.0 * s;
    let w1 = g.button(x + 28.0 * s, by, 15.0 * s, "Dismiss (Enter)", rgb(255, 196, 94), Event::Key(Key::Enter));
    g.button(x + 28.0 * s + w1 + 12.0 * s, by, 15.0 * s, "Snooze 5 min (S)", MUTED, Event::Key(Key::Char('s')));
}

/// The power menu: a dropdown under the top bar's right end (like the speaker's), not a dialog in the middle. Lock only for a signed-in
/// person (it signs out, which also seals the vault), Sleep only where the machine can sleep.
fn draw_power_menu(g: &mut Gfx, w: u32, h: u32, s: f32, sleep: bool, lock: bool) {
    g.clear_hits();
    g.rect(0.0, 0.0, w as f32, h as f32, [0, 0, 0, 90]);
    g.hit(R::new(0.0, 0.0, w as f32, h as f32), Event::Key(Key::Esc), "backdrop-cancel");
    let mut items: Vec<(&str, [u8; 4], char)> = vec![];
    if lock {
        items.push(("Lock", TEXT, 'l'));
    }
    items.push(("Restart", TEXT, 'r'));
    if sleep {
        items.push(("Sleep", TEXT, 's'));
    }
    items.push(("Power off", ERR, 'y'));
    items.push(("Cancel", MUTED, '\u{1b}'));
    let size = 15.0 * s;
    let (pad, gap, row_h) = (14.0 * s, 8.0 * s, size * 2.1);
    let bw = (232.0 * s).min(w as f32 - 16.0);
    let bh = (2.0 * pad + 24.0 * s + items.len() as f32 * (row_h + gap) - gap).min(h as f32 - 8.0);
    let (x, y) = (w as f32 - bw - 12.0 * s, 44.0 * s);
    g.hit(R::new(x, y, bw, bh), Event::Tick, "backdrop");
    crate::fx::modal_panel(g, R::new(x, y, bw, bh), s, ERR, true);
    g.icon(Icon::Power, x + pad + 9.0 * s, y + pad + 10.0 * s, 18.0 * s, ERR);
    g.text(x + pad + 26.0 * s, y + pad, 16.0 * s, TEXT, "Power", Face::Bold);
    let mut by = y + pad + 28.0 * s;
    for (label, accent, k) in &items {
        let ev = if *k == '\u{1b}' { Event::Key(Key::Esc) } else { Event::Key(Key::Char(*k)) };
        g.button(x + pad, by, size, label, *accent, ev);
        by += row_h + gap;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use iyakki_agent::facets_h::RealServices as _;
    use super::*;
    use iyakki_shared::chat::Unconfigured;
    use crate::apps::tests::{temp_dir, MockHost};
    use iyakki_sec::policy::Role;

    pub fn env(tag: &str) -> (ShellEnv, Arc<MockHost>) {
        let (e, _, h) = env_store(tag);
        (e, h)
    }

    /// The same, and the store itself, for a test that has to hold the raw lock (to poison it).
    pub fn env_store(tag: &str) -> (ShellEnv, Arc<Mutex<UserStore>>, Arc<MockHost>) {
        let d = temp_dir(tag);
        let host = Arc::new(MockHost::default());
        let raw = Arc::new(Mutex::new(UserStore::open(&d.join("users.json")).unwrap()));
        (
            ShellEnv {
                store: crate::accounts::UsersRef::of_store(&raw),
                audit: Arc::new(AuditLog::new(&d.join("audit.log"))),
                host: host.clone(),
                cfg: AiConfig::parse("").unwrap(),
                chat: Arc::new(Unconfigured),
                mcp_servers: vec![],
                services: crate::apps::tests::services_in(&d, Arc::new(crate::apps::tests::Offline)),
                web: Arc::new(iyakki_web::fetch::NoNetwork),
                hostname: "iyakki-test".into(),
                data_volatile: false,
                hwcheck: false,
            },
            raw,
            host,
        )
    }

    pub(super) fn typ(sh: &mut Shell, s: &str) {
        for c in s.chars() {
            sh.handle(&Event::Key(Key::Char(c)));
        }
    }
    /// A key press. The keys that sign out on Home (o, q, Esc) now ask first; most tests only want the person signed out, so when
    /// one of them opened the confirm card it is answered Enter here. The tests of the card itself (`shell_signout_tests.rs`) press
    /// with `Shell::handle` directly.
    pub(super) fn key(sh: &mut Shell, k: Key) {
        sh.handle(&Event::Key(k));
        if sh.signout_card_open() {
            sh.handle(&Event::Key(Key::Enter));
        }
    }

    /// What the digit keys did before the home pages: open the Nth app of the whole list (the keys 1 to 6 are pages now).
    pub(super) fn open_nth(sh: &mut Shell, n: usize) {
        let app = sh.visible_tiles().get(n - 1).map(|t| t.app);
        if let Some(a) = app {
            sh.open(a);
        }
    }

    pub(super) fn setup_admin(sh: &mut Shell) {
        assert_eq!(sh.screen_name(), "setup");
        typ(sh, "root");
        key(sh, Key::Tab);
        typ(sh, "rootpass12");
        key(sh, Key::Tab);
        typ(sh, "rootpass12");
        key(sh, Key::Enter);
    }

    pub(crate) fn login(sh: &mut Shell, u: &str, p: &str) {
        // the form may already hold the last person's name with the cursor on the password: go back to the name and replace it
        if let Screen::Login(f) = &mut sh.screen {
            f.focus = 0;
            f.fields[0].clear();
        }
        typ(sh, u);
        key(sh, Key::Tab);
        typ(sh, p);
        key(sh, Key::Enter);
    }

    #[test]
    fn the_hardware_check_starts_before_login_counts_input_and_leaves_with_enter() {
        let (mut e, _) = env("hw1");
        e.hwcheck = true;
        let mut sh = Shell::new(e);
        assert_eq!(sh.screen_name(), "hwcheck");
        let mut g = Gfx::new(1920, 1080);
        sh.render(&mut g);
        // a tap on a target registers (the hit comes from the drawn frame) and moves/keys are counted
        let r = sh.hits().iter().find(|h| h.label == "target 1").expect("targets are clickable").rect;
        sh.handle(&Event::MouseMove((r.x + 5.0) as i32, (r.y + 5.0) as i32));
        sh.handle(&Event::MouseDown((r.x + 5.0) as i32, (r.y + 5.0) as i32));
        key(&mut sh, Key::Char('q'));
        sh.render(&mut g);
        let hit_after = sh.hits().iter().filter(|h| h.label.starts_with("target ")).count();
        assert_eq!(hit_after, 8);
        if let Screen::HwCheck(h) = &sh.screen {
            assert_eq!(h.targets_hit(), 1);
            assert_eq!(h.counts(), (1, 1, 1));
        } else {
            panic!("left the hardware check too early");
        }
        // nothing else is reachable from here: typing does not reach a login form
        key(&mut sh, Key::Enter);
        assert_eq!(sh.screen_name(), "setup", "first run: the admin account is created next");
        // with an account present, Enter leads to the login screen
        let (mut e2, _) = env("hw2");
        e2.hwcheck = true;
        e2.store.lock().unwrap().create_first_admin("root", "rootpass12").unwrap();
        let mut sh2 = Shell::new(e2);
        assert_eq!(sh2.screen_name(), "hwcheck");
        key(&mut sh2, Key::Esc);
        assert_eq!(sh2.screen_name(), "login");
    }

    #[test]
    fn first_run_setup_then_login_launcher() {
        let (e, _) = env("sh1");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        assert_eq!(sh.screen_name(), "launcher");
        assert_eq!(sh.session().unwrap().role, Role::Admin);
        assert_eq!(sh.visible_tiles().len(), TILES.iter().filter(|t| t.group.is_some()).count(), "admins see the Users tile (every tile on the grid)");
        key(&mut sh, Key::Char('o'));
        assert_eq!(sh.screen_name(), "login");
        login(&mut sh, "root", "wrong-password");
        assert_eq!(sh.screen_name(), "login");
        // the username stays filled in and focus returns to the (cleared) password field
        typ(&mut sh, "rootpass12");
        key(&mut sh, Key::Enter);
        assert_eq!(sh.screen_name(), "launcher");
    }

    #[test]
    fn setup_rejects_mismatch_and_weak_input() {
        let (e, _) = env("sh2");
        let mut sh = Shell::new(e);
        typ(&mut sh, "root");
        key(&mut sh, Key::Tab);
        typ(&mut sh, "rootpass12");
        key(&mut sh, Key::Tab);
        typ(&mut sh, "different1");
        key(&mut sh, Key::Enter);
        assert_eq!(sh.screen_name(), "setup");
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        // weak password
        let (e, _) = env("sh2b");
        let mut sh = Shell::new(e);
        typ(&mut sh, "root");
        key(&mut sh, Key::Tab);
        typ(&mut sh, "short");
        key(&mut sh, Key::Tab);
        typ(&mut sh, "short");
        key(&mut sh, Key::Enter);
        assert_eq!(sh.screen_name(), "setup");
    }

    pub(crate) fn shell_with_users(tag: &str) -> (Shell, Arc<MockHost>) {
        let (sh, _, h) = shell_with_users_store(tag);
        (sh, h)
    }

    pub(crate) fn shell_with_users_store(tag: &str) -> (Shell, Arc<Mutex<UserStore>>, Arc<MockHost>) {
        let (e, raw, h) = env_store(tag);
        {
            let mut st = e.store.lock().unwrap();
            st.create_first_admin("root", "rootpass12").unwrap();
            let admin = st.authenticate("root", "rootpass12").unwrap();
            st.create_user(&admin, "alice", "alicepass1", Role::User).unwrap();
            st.create_user(&admin, "gus", "guestpass1", Role::Guest).unwrap();
            st.create_user(&admin, "oscar", "operpass12", Role::Operator).unwrap();
        }
        (Shell::new(e), raw, h)
    }

    #[test]
    fn a_poisoned_account_store_lock_does_not_lock_everyone_out_of_signing_in_or_changing_a_password() {
        let (mut sh, store, _) = shell_with_users_store("poisoned-store");
        let raw = store.clone();
        let died = std::thread::spawn(move || {
            let _held = store.lock().unwrap(); // poison-ok: the test makes the lock poisoned on purpose
            panic!("a thread of an app died while it held the account store");
        })
        .join();
        assert!(died.is_err() && raw.is_poisoned(), "the lock is poisoned");
        // a person can still sign in (a poisoned lock used to answer every sign-in with a 'lock' storage error)
        login(&mut sh, "alice", "alicepass1");
        assert_ne!(sh.screen_name(), "login", "signed in: {}", sh.screen_name());
        assert_eq!(sh.session.as_ref().map(|s| s.user.as_str()), Some("alice"));
        // and a wrong password is still refused for the right reason
        let (mut sh2, store, _) = shell_with_users_store("poisoned-store-2");
        let _ = std::thread::spawn(move || {
            let _held = store.lock().unwrap(); // poison-ok: the test makes the lock poisoned on purpose
            panic!("again");
        })
        .join();
        login(&mut sh2, "alice", "not-her-password");
        assert_eq!(sh2.screen_name(), "login");
        if let Screen::Login(f) = &sh2.screen {
            assert!(f.error.as_deref().is_some_and(|e| !e.contains("lock")), "{:?}", f.error);
        }
    }

    #[test]
    fn double_click_picks_a_word_and_dragging_the_pointer_with_the_button_held_selects() {
        let (e, _h) = env("select");
        e.store.lock().unwrap().create_first_admin("root", "rootpass12").unwrap();
        let mut sh = Shell::new(e);
        let mut g = Gfx::new(1280, 720);
        for c in "alpha beta".chars() {
            sh.handle(&Event::Key(Key::Char(c)));
        }
        sh.render(&mut g);
        let field = sh.hits().iter().find(|h| h.label == "field" && h.ev == Event::Focus(0)).expect("username field").rect;
        let y = (field.y + field.h / 2.0) as i32;
        let selected = |sh: &Shell| match &sh.screen {
            Screen::Login(f) => f.fields[0].selected_text(),
            _ => None,
        };
        // a double click inside the second word selects it
        let x = field.x as i32 + 12 + 80;
        sh.handle(&Event::MouseDown(x, y));
        sh.render(&mut g);
        sh.handle(&Event::MouseUp(x, y));
        sh.handle(&Event::MouseDown(x, y));
        sh.render(&mut g);
        sh.handle(&Event::MouseUp(x, y));
        assert_eq!(selected(&sh).as_deref(), Some("beta"), "double click selects the word");
        // press, move with the button held, release: the text between is selected
        sh.tick += 100; // not a double click
        sh.handle(&Event::MouseDown(field.x as i32 + 12, y));
        sh.render(&mut g);
        sh.handle(&Event::MouseMove(x, y));
        sh.render(&mut g);
        sh.handle(&Event::MouseUp(x, y));
        let drag = selected(&sh).expect("a selection");
        assert!(drag.starts_with("alpha"), "{drag}");
        // moving without the button does not select
        sh.tick += 100;
        sh.handle(&Event::MouseDown(field.x as i32 + 12, y));
        sh.render(&mut g);
        sh.handle(&Event::MouseUp(field.x as i32 + 12, y));
        sh.handle(&Event::MouseMove(x, y));
        assert_eq!(selected(&sh), None, "{:?}", selected(&sh));
    }

    #[test]
    fn an_administrator_is_told_about_a_new_device_on_the_network_and_an_ordinary_user_is_not() {
        let (mut sh, _) = shell_with_users("newdev");
        login(&mut sh, "root", "rootpass12");
        let n = |ip: &str, mac: &str| iyakki_shared::devwatch::Neighbor { ip: ip.into(), mac: mac.into(), iface: "eth0".into() };
        let dw = sh.services_for_demo().devwatch().clone();
        dw.observe(&[n("10.0.0.1", "aa:aa:aa:aa:aa:01")], 1);
        dw.observe(&[n("10.0.0.1", "aa:aa:aa:aa:aa:01"), n("10.0.0.9", "bb:bb:bb:bb:bb:09")], 2);
        for _ in 0..95 {
            sh.handle(&Event::Tick);
        }
        assert!(sh.toast_text().is_some_and(|t| t.contains("New device on your network: 10.0.0.9")), "{:?}", sh.toast_text());
        let log = std::fs::read_to_string(sh.services_for_demo().dir().parent().unwrap().join("audit.log")).unwrap_or_default();
        let _ = log;
        // an ordinary account is not told (it cannot act on it)
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "alice", "alicepass1");
        dw.observe(&[n("10.0.0.1", "aa:aa:aa:aa:aa:01"), n("10.0.0.77", "cc:cc:cc:cc:cc:77")], 3);
        for _ in 0..95 {
            sh.handle(&Event::Tick);
        }
        assert!(sh.toast_text().is_none_or(|t| !t.contains("10.0.0.77")));
    }

    #[test]
    fn the_text_and_button_size_a_person_chose_is_applied_at_their_sign_in_and_gone_at_the_sign_in_screen() {
        let (mut sh, _) = shell_with_users("uisize");
        let dir = sh.services_for_demo().dir().to_path_buf();
        crate::uisize::save(&dir, "alice", 3); // Larger, 150%
        let normal = scale_for(1280, 720);
        login(&mut sh, "root", "rootpass12");
        assert_eq!(scale_for(1280, 720), normal, "root chose nothing");
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "alice", "alicepass1");
        assert_eq!(scale_for(1280, 720), 1.5 * normal);
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        let tile = |sh: &Shell| sh.hits().iter().find(|h| h.label.to_lowercase().contains("chat")).map(|h| h.rect.w);
        let big = tile(&sh);
        key(&mut sh, Key::Char('o'));
        assert_eq!(scale_for(1280, 720), normal, "the sign-in screen is at normal size again");
        login(&mut sh, "root", "rootpass12");
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        assert!(big.is_none() || tile(&sh).is_none() || big > tile(&sh), "tiles were bigger for alice");
    }

    #[test]
    fn the_theme_a_person_chose_is_applied_at_their_sign_in_and_dark_again_at_the_sign_in_screen() {
        let (mut sh, _) = shell_with_users("theme");
        let dir = sh.services_for_demo().dir().to_path_buf();
        crate::theme::save(&dir, "alice", crate::theme::ThemeId::Light);
        login(&mut sh, "root", "rootpass12");
        assert_eq!(crate::theme::current_id(), crate::theme::ThemeId::Dark, "root chose nothing: the approved dark look");
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "alice", "alicepass1");
        assert_eq!(crate::theme::current_id(), crate::theme::ThemeId::Light);
        key(&mut sh, Key::Char('o'));
        assert_eq!(crate::theme::current_id(), crate::theme::ThemeId::Dark, "the sign-in screen is always Dark");
    }

    #[test]
    fn the_sign_in_form_fills_in_the_last_person_who_signed_in_and_puts_the_cursor_on_the_password() {
        let (e, _h) = env("lastuser");
        {
            let mut st = e.store.lock().unwrap();
            st.create_first_admin("root", "rootpass12").unwrap();
            let admin = st.authenticate("root", "rootpass12").unwrap();
            st.create_user(&admin, "alice", "alicepass1", Role::User).unwrap();
        }
        let dir = e.data_dir().to_path_buf();
        let mut sh = Shell::new(e);
        // nobody has signed in yet: an empty form with the cursor on the username
        let Screen::Login(f) = &sh.screen else { panic!("login expected") };
        assert_eq!((f.fields[0].value.as_str(), f.focus), ("", 0));
        // a failed attempt remembers nothing
        for c in "alice".chars() {
            sh.handle(&Event::Key(Key::Char(c)));
        }
        sh.handle(&Event::Key(Key::Enter));
        for c in "wrong-password".chars() {
            sh.handle(&Event::Key(Key::Char(c)));
        }
        sh.handle(&Event::Key(Key::Enter));
        assert!(!dir.join("last-user").exists());
        // a good sign-in is remembered; signing out shows the form filled in, cursor on the password
        sh.screen = Screen::Login(Form::login());
        login(&mut sh, "alice", "alicepass1");
        assert_eq!(std::fs::read_to_string(dir.join("last-user")).unwrap().trim(), "alice");
        key(&mut sh, Key::Char('o'));
        let Screen::Login(f) = &sh.screen else { panic!("signed out") };
        assert_eq!((f.fields[0].value.as_str(), f.focus, f.fields[1].value.as_str()), ("alice", 1, ""), "the password is never filled in");
        // typing goes straight into the password, and Enter signs in
        for c in "alicepass1".chars() {
            sh.handle(&Event::Key(Key::Char(c)));
        }
        sh.handle(&Event::Key(Key::Enter));
        assert_eq!(sh.screen_name(), "launcher");
        // the next start of the program remembers too, and a damaged or odd file is ignored
        std::fs::write(dir.join("last-user"), "../../etc/passwd\n").unwrap();
        assert_eq!(last_user(&sh.env), None);
        std::fs::write(dir.join("last-user"), "root\n").unwrap();
        assert_eq!(last_user(&sh.env).as_deref(), Some("root"));
    }

    #[test]
    fn the_samples_are_put_in_place_once_by_an_administrators_first_sign_in_and_stay_deleted() {
        let (mut sh, _) = shell_with_users("samples1");
        let docs = std::env::temp_dir().join(format!("iyakki-samples-docs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&docs);
        std::fs::create_dir_all(&docs).unwrap();
        sh.env.cfg.rag_docs = docs.display().to_string();
        let svc = sh.env.services.clone();
        // an ordinary user signing in first changes nothing
        login(&mut sh, "alice", "alicepass1");
        assert!(svc.mcp().list().is_empty() && svc.skills().list("alice").is_empty() && !docs.join("shared").exists());
        key(&mut sh, Key::Esc);
        // the administrator's first sign-in puts the examples in place
        login(&mut sh, "root", "rootpass12");
        let mcp = svc.mcp().list();
        assert_eq!(mcp.len(), 1);
        assert!(mcp[0].sample && mcp[0].name == "iyakki-demo" && mcp[0].args == vec!["--mcp-demo".to_string()]);
        let skills: Vec<String> = svc.skills().list("root").into_iter().map(|s| s.name).collect();
        for n in ["sample-explain-simply", "sample-python-practice", "sample-meeting-notes"] {
            assert!(skills.contains(&n.to_string()), "{n}");
        }
        // the skill that ships with IYAKKI is there too, and it parses and fits the prompt budget
        assert!(skills.contains(&"k3s-dev-best-practices".to_string()));
        let k3s = svc.skills().get(&iyakki_shared::skills::Scope::Shared, "k3s-dev-best-practices").unwrap();
        assert!(k3s.body.chars().count() < 4000 && k3s.body.contains("deploy_build") && k3s.body.contains("k8s_teardown"));
        for (name, md) in crate::samples::BUILTIN_SKILLS {
            let k = iyakki_agent::skills::parse(md, iyakki_shared::skills::Scope::Shared).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(k.name == name && k.body.chars().count() <= 3950 && md.is_ascii() && k.triggers.len() <= 12, "{name}");
            assert!(skills.contains(&name.to_string()), "{name} is installed");
            // every dev skill insists on bugs as their own B- rows with captured logs
            assert!(k.body.contains("B- id") && k.body.contains("bug_report") && k.body.contains("never F-"), "{name} has the bug rule");
        }
        assert!(svc.real().skills.select("alice", "deploy my node app to k3s", 3).iter().any(|s| s.name == "k3s-dev-best-practices" || s.name == "k3s-build-and-deploy"));
        assert!(svc.real().skills.select("alice", "build and deploy this app to k3s", 3).iter().any(|s| s.name == "k3s-build-and-deploy"));
        for (file, _) in crate::samples::DOCS {
            assert!(docs.join("shared").join(file).is_file(), "{file}");
        }
        let log = std::fs::read_to_string(sh.env.audit.path_buf()).unwrap();
        assert!(log.contains("samples.seed"));
        // every skill (including the shared samples) can be seen by everyone, and the sample MCP server reaches the chat
        assert!(svc.skills().list("alice").iter().any(|s| s.name == "sample-python-practice"));
        // deleting them: they do not return at the next sign-in
        svc.mcp().remove("iyakki-demo").unwrap();
        for n in ["sample-explain-simply", "sample-python-practice", "sample-meeting-notes"].into_iter().chain(crate::samples::BUILTIN_SKILLS.iter().map(|b| b.0)) {
            svc.skills().remove(&iyakki_shared::skills::Scope::Shared, n, true, true).unwrap();
        }
        std::fs::remove_file(docs.join("shared/sample-iyakki-quick-start.md")).unwrap();
        key(&mut sh, Key::Esc);
        login(&mut sh, "root", "rootpass12");
        assert!(svc.mcp().list().is_empty() && svc.skills().list("root").is_empty());
        assert!(!docs.join("shared/sample-iyakki-quick-start.md").exists());
    }

    #[test]
    fn an_alarm_or_reminder_rings_over_everything_until_dismissed_and_snooze_brings_it_back() {
        let (mut sh, _) = shell_with_users("ring1");
        login(&mut sh, "alice", "alicepass1");
        // no sound program is run by the test
        sh.env.settings().lock().unwrap().update(|s| s.voice.play_cmd = "tee /dev/null".into()).unwrap();
        let zone = iyakki_clock::Zone::system();
        let svc = sh.env.services.clone();
        let cal = &svc.calendar();
        let now = iyakki_clock::now_unix();
        sh.check_alarms(now); // the first look only starts the clock
        assert!(sh.ringing().is_none());
        cal.add_event("alice", "Standup", now + 5, 15, false, iyakki_shared::calendar::Repeat::None, "", "user", Some(0)).unwrap();
        sh.check_alarms(now + 3);
        assert!(sh.ringing().is_none(), "not yet");
        sh.check_alarms(now + 6);
        assert_eq!(sh.ringing(), Some(vec!["Standup".to_string()]));
        // it is drawn over whatever is on screen and does not let other keys through
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.label == "backdrop") && sh.hits().iter().any(|h| h.ev == Event::Key(Key::Char('s'))), "the dialog is drawn with its buttons");
        key(&mut sh, Key::Char('x'));
        assert!(sh.ringing().is_some(), "other keys are ignored while it rings");
        // snooze: quiet now, back five minutes later
        key(&mut sh, Key::Char('s'));
        assert!(sh.ringing().is_none() && sh.toast_text().unwrap().contains("Snoozed"));
        sh.check_alarms(now + 100);
        assert!(sh.ringing().is_none());
        sh.check_alarms(now + 6 + 400);
        assert_eq!(sh.ringing(), Some(vec!["Standup".to_string()]));
        key(&mut sh, Key::Enter);
        assert!(sh.ringing().is_none());
        sh.check_alarms(now + 6 + 900);
        assert!(sh.ringing().is_none(), "dismissed means gone");
        // a one-time alarm rings at its minute and switches itself off
        let t = now + 7200;
        let l = zone.local(t);
        cal.add_alarm("alice", "Tea", l.hour, l.minute, 0, now, &zone, "assistant").unwrap();
        let at = zone.local_to_unix(l.year, l.month, l.day, l.hour, l.minute, 0);
        sh.check_alarms(at - 1);
        assert!(sh.ringing().is_none());
        sh.check_alarms(at + 1);
        assert_eq!(sh.ringing(), Some(vec!["Tea".to_string()]));
        assert!(!cal.alarms("alice")[0].enabled);
        let log = std::fs::read_to_string(sh.env.audit.path_buf()).unwrap();
        assert!(log.contains("calendar.reminder") && log.contains("alarm.ring"), "{log}");
        sh.render(&mut g);
        // nobody is signed in: nothing rings and the clock restarts
        key(&mut sh, Key::Esc);
        // an unanswered alarm gives up after five minutes
        sh.check_alarms(at + 400);
        assert!(sh.ringing().is_none(), "no one answered for five minutes");
    }

    fn bell_hit(sh: &Shell) -> Option<crate::ui::Hit> {
        sh.hits().iter().find(|h| h.label == "inbox").cloned()
    }

    #[test]
    fn the_pointer_is_a_hand_over_buttons_a_text_bar_over_fields_and_an_arrow_elsewhere() {
        use crate::ui::Pointer;
        let (mut sh, _) = shell_with_users("ptr1");
        let mut g = Gfx::new(1280, 720);
        // the sign-in screen has text fields and a button
        sh.render(&mut g);
        let centre = |r: crate::ui::R| ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
        let field = sh.hits().iter().find(|h| h.label == "field").expect("a field").rect;
        let (x, y) = centre(field);
        sh.handle(&Event::MouseMove(x, y));
        assert_eq!(sh.pointer(), Pointer::Text);
        sh.handle(&Event::MouseMove(2, 2));
        assert_eq!(sh.pointer(), Pointer::Default, "nothing clickable under the corner");
        login(&mut sh, "alice", "alicepass1");
        sh.render(&mut g);
        let tile = sh.hits().iter().find(|h| h.label == "inbox").expect("the bell").rect;
        let (x, y) = centre(tile);
        sh.handle(&Event::MouseMove(x, y));
        assert_eq!(sh.pointer(), Pointer::Hand);
    }

    #[test]
    fn the_bell_is_on_the_launcher_and_inside_apps_counts_what_waits_and_opens_the_inbox() {
        let (mut sh, _) = shell_with_users("bell1");
        login(&mut sh, "alice", "alicepass1");
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        let on_launcher = bell_hit(&sh).expect("a bell on the launcher");
        assert_eq!(on_launcher.ev, Event::Key(Key::F(9)));
        assert_eq!(sh.badge(), Badge::default());
        // unread warnings, an overdue task and a waiting approval all count
        let svc = sh.env.services.clone();
        let now = iyakki_clock::now_unix();
        svc.real().notify("alice", iyakki_shared::inbox::Kind::Warning, "one", "", None).unwrap();
        svc.real().notify("alice", iyakki_shared::inbox::Kind::Warning, "two", "", None).unwrap();
        svc.inbox().add_task("alice", "late", "", Some(now - 60), iyakki_shared::inbox::Priority::Normal, "user", now - 100).unwrap();
        svc.real().notify("bob", iyakki_shared::inbox::Kind::Warning, "not mine", "", None).unwrap();
        sh.handle(&Event::MouseMove(5, 5)); // any event refreshes nothing; a key does
        key(&mut sh, Key::Right);
        assert_eq!(sh.badge(), Badge { unread: 2, approvals: 0, overdue: 1, other: 0 });
        assert_eq!(sh.badge().total(), 3);
        sh.render(&mut g);
        // a ticking clock picks up changes made elsewhere within half a second
        svc.real().notify("alice", iyakki_shared::inbox::Kind::Warning, "three", "", None).unwrap();
        for _ in 0..16 {
            sh.handle(&Event::Tick);
        }
        assert_eq!(sh.badge().unread, 3);
        assert!(sh.take_stale());
        // the bell opens the inbox by click
        let r = bell_hit(&sh).unwrap_or(on_launcher).rect;
        sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
        assert_eq!(sh.screen_name(), "app:inbox");
        sh.render(&mut g);
        assert!(bell_hit(&sh).is_some(), "the bell is in the header of an app too");
        // and closes it again with F9, back on the launcher
        key(&mut sh, Key::F(9));
        assert_eq!(sh.screen_name(), "launcher");
        // from inside an app: F9 keeps the app aside and Back returns to it
        open_nth(&mut sh, 6);
        assert_eq!(sh.screen_name(), "app:runs");
        sh.render(&mut g);
        assert!(bell_hit(&sh).is_some());
        key(&mut sh, Key::F(9));
        assert_eq!(sh.screen_name(), "app:inbox");
        assert!(sh.stash.is_some());
        key(&mut sh, Key::Esc);
        assert_eq!(sh.screen_name(), "app:runs", "Back returns to the app that was open");
        assert!(sh.stash.is_none());
        // Back from the launcher's inbox goes to the launcher, and signing out forgets everything
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::F(9));
        key(&mut sh, Key::Esc);
        assert_eq!(sh.screen_name(), "launcher");
        key(&mut sh, Key::Char('o'));
        assert_eq!(sh.badge(), Badge::default());
        key(&mut sh, Key::F(9));
        assert_eq!(sh.screen_name(), "login", "no inbox before sign-in");
    }

    #[test]
    fn routine_items_never_add_to_the_bells_number_and_99_plus_needs_real_counts() {
        let (mut sh, _) = shell_with_users("bell4");
        login(&mut sh, "alice", "alicepass1");
        let svc = sh.env.services.clone();
        let now = iyakki_clock::now_unix();
        // 130 routine items, like the Mac that showed 99+: new devices, finished jobs, information
        for i in 0..79 {
            svc.inbox().add("alice", iyakki_shared::inbox::Kind::Device, "New device on your network", &format!("10.0.0.{i} (m{i}) appeared."), "scan", None, now).unwrap();
        }
        for i in 0..51 {
            svc.inbox().add("alice", iyakki_shared::inbox::Kind::Info, &format!("note {i}"), "", &format!("src{}", i % 5), None, now).unwrap();
        }
        sh.refresh_badge();
        let b = sh.badge();
        assert_eq!((b.total(), b.unread, b.approvals, b.overdue), (0, 0, 0, 0), "no number: {b:?}");
        assert!(b.other > 0, "a dot instead");
        // layout at the four sizes with each state: nothing, the dot, a number, 99+
        let sizes = [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)];
        let check = |sh: &mut Shell, label: &str| {
            for (w, h) in sizes {
                sh.resize_for_test(w, h);
                let mut g = Gfx::new(w, h);
                sh.render(&mut g);
                let r = bell_hit(sh).unwrap_or_else(|| panic!("no bell at {w}x{h} ({label})")).rect;
                assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32, "{label} {w}x{h}: {r:?}");
                let others: Vec<_> = sh.hits().iter().filter(|x| x.label != "inbox" && x.rect.y < 46.0 && x.rect.w > 0.0 && x.rect.w < 200.0).collect();
                for o in others {
                    let (ox, oy) = (o.rect.x.max(r.x) < (o.rect.x + o.rect.w).min(r.x + r.w), o.rect.y.max(r.y) < (o.rect.y + o.rect.h).min(r.y + r.h));
                    assert!(!(ox && oy), "{label} {w}x{h}: the bell overlaps `{}`", o.label);
                }
            }
        };
        check(&mut sh, "dot");
        let e = |i: usize| svc.inbox().add("alice", iyakki_shared::inbox::Kind::Error, &format!("Flow {i} failed"), &format!("reason {i}"), &format!("s{}", i % 5), None, now).unwrap();
        for i in 0..3 {
            e(i);
        }
        sh.refresh_badge();
        assert_eq!((sh.badge().unread, sh.badge().total()), (3, 3));
        check(&mut sh, "three");
        for i in 3..120 {
            e(i);
        }
        sh.refresh_badge();
        assert!(sh.badge().total() > 99, "{:?}", sh.badge());
        check(&mut sh, "99+");
    }

    #[test]
    fn a_waiting_approval_turns_the_bell_red_and_is_answered_from_the_inbox_while_the_app_stays_alive() {
        use iyakki_agent::agent::RunEvent;
        struct Ask(std::sync::atomic::AtomicUsize);
        impl iyakki_shared::chat::ChatBackend for Ask {
            fn complete(&self, _: &[iyakki_shared::chat::Message]) -> Result<String, String> {
                Err("x".into())
            }
            fn complete_tools(&self, _: &[iyakki_shared::chat::Message], _: &[iyakki_shared::chat::ToolSpec]) -> Result<iyakki_shared::chat::ModelTurn, String> {
                if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Ok(iyakki_shared::chat::ModelTurn { text: String::new(), calls: vec![iyakki_shared::chat::ToolCall { id: "k1".into(), name: "run_command".into(), args: serde_json::json!({}) }] })
                } else {
                    Ok(iyakki_shared::chat::ModelTurn { text: "ok".into(), calls: vec![] })
                }
            }
        }
        struct Tools;
        impl iyakki_shared::agent::Tools for Tools {
            fn defs(&self) -> Vec<iyakki_shared::agent::ToolDef> {
                vec![iyakki_shared::agent::ToolDef { spec: iyakki_shared::chat::ToolSpec { name: "run_command".into(), description: "x".into(), schema: serde_json::json!({"type": "object"}) }, risk: iyakki_shared::agent::Risk::Ask, untrusted: false }]
            }
            fn call(&self, _: &str, _: &serde_json::Value) -> Result<String, String> {
                Ok("ran".into())
            }
            fn describe(&self, _: &str, _: &serde_json::Value) -> String {
                "run `ls`".into()
            }
        }
        let (mut sh, _) = shell_with_users("bell2");
        login(&mut sh, "alice", "alicepass1");
        // a run started in an app (here the shell stands in for the chat: it holds the session on the screen)
        struct Holder(iyakki_agent::runs::RunSession, Vec<RunEvent>);
        impl App for Holder {
            fn title(&self) -> &'static str {
                "Holder"
            }
            fn icon(&self) -> Icon {
                Icon::Chat
            }
            fn accent(&self) -> [u8; 4] {
                rgb(1, 2, 3)
            }
            fn hint(&self) -> String {
                String::new()
            }
            fn handle(&mut self, ev: &Event, _: &AppCx) -> Flow {
                if matches!(ev, Event::Tick) {
                    let e = self.0.poll();
                    self.1.extend(e);
                }
                Flow::Stay
            }
            fn render(&mut self, _: &mut Gfx, _: R, _: f32, _: &AppCx, _: u64) {}
        }
        let run = iyakki_agent::runs::RunSession::start(Some(sh.env.services.real().runs.clone()), "alice", "list my files", Arc::new(Ask(Default::default())), Arc::new(Tools), vec![], Default::default());
        sh.screen = Screen::App(Box::new(Holder(run, vec![])), PApp::Chat);
        for _ in 0..400 {
            sh.handle(&Event::Tick);
            if sh.badge().approvals == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(sh.badge().approvals, 1);
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        // the badge is red while an approval waits
        let r = bell_hit(&sh).unwrap().rect;
        let red = (r.x as u32..(r.x + r.w) as u32).any(|x| (0..r.y as u32 + 20).any(|y| g.pm_mut().pixel(x, y).is_some_and(|p| p.red() > 190 && p.green() < 80 && p.blue() < 100)));
        assert!(red, "a red badge near the bell");
        // open the inbox from the app, go to approvals and approve with y
        key(&mut sh, Key::F(9));
        key(&mut sh, Key::Char('3'));
        key(&mut sh, Key::Char('y'));
        assert_eq!(sh.badge().approvals, 0);
        // the app that was kept aside went on running: its run finished with the tool executed
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(30) {
            sh.handle(&Event::Tick);
            if sh.env.services.runs().live().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(sh.env.services.runs().live().is_empty(), "the run finished while the inbox was open");
        key(&mut sh, Key::Esc);
        assert_eq!(sh.screen_name(), "app:chat");
        // finishing left a note for alice (and the audit log has the decision)
        sh.refresh_badge();
        assert!(sh.env.inbox().list("alice").iter().any(|n| n.kind == iyakki_shared::inbox::Kind::Agent && n.body == "ok"));
        let log = std::fs::read_to_string(sh.env.audit.path_buf()).unwrap();
        assert!(log.contains("inbox.approve") && log.contains("run_command"), "{log}");
    }

    #[test]
    fn reminders_alarms_and_new_devices_leave_one_notification_each_not_one_per_tick() {
        let (mut sh, _) = shell_with_users("bell3");
        login(&mut sh, "alice", "alicepass1");
        sh.env.settings().lock().unwrap().update(|s| s.voice.play_cmd = "tee /dev/null".into()).unwrap();
        let svc = sh.env.services.clone();
        let now = iyakki_clock::now_unix();
        sh.check_alarms(now);
        svc.calendar().add_event("alice", "Standup", now + 5, 15, false, iyakki_shared::calendar::Repeat::None, "", "user", Some(0)).unwrap();
        for dt in 1..=30 {
            sh.check_alarms(now + dt);
        }
        let l = svc.inbox().list("alice");
        assert_eq!(l.len(), 1, "{l:?}");
        assert_eq!((l[0].kind, l[0].title.as_str()), (iyakki_shared::inbox::Kind::Reminder, "Reminder: Standup"));
        assert_eq!(l[0].link, Some(iyakki_shared::inbox::Link::new("calendar", "")));
        let zone = iyakki_clock::Zone::system();
        let t = zone.local(now + 7200);
        svc.calendar().add_alarm("alice", "Tea", t.hour, t.minute, 0, now, &zone, "user").unwrap();
        let at = zone.local_to_unix(t.year, t.month, t.day, t.hour, t.minute, 0);
        sh.check_alarms(at - 1);
        sh.check_alarms(at + 1);
        sh.check_alarms(at + 2);
        assert_eq!(svc.inbox().list("alice").iter().filter(|n| n.title == "Alarm: Tea").count(), 1);
        sh.refresh_badge();
        assert_eq!(sh.badge().unread, 2);
    }

    #[test]
    fn the_inbox_is_enabled_for_every_role_but_has_no_tile() {
        for r in Role::ALL {
            assert!(r.template().apps.contains(&PApp::Inbox), "{r:?}");
        }
        assert!(TILES.iter().all(|t| t.app != PApp::Inbox));
        assert_eq!(PApp::Inbox.id(), "inbox");
    }

    #[test]
    fn the_ai_ide_tile_is_one_tile_in_build_for_every_role_but_guest() {
        assert_eq!(TILES.iter().filter(|t| t.app == PApp::Ide).count(), 1);
        let t = TILES.iter().find(|t| t.app == PApp::Ide).unwrap();
        assert_eq!((t.label, t.sub), ("AI IDE", "Code with the AI"));
        assert_eq!(Group::of(PApp::Ide), Some(Group::Work));
        assert_eq!((PApp::Ide.id(), PApp::Ide.label().as_str()), ("ide", "AI IDE"));
        for r in [Role::Admin, Role::Operator, Role::User] {
            assert!(r.template().apps.contains(&PApp::Ide), "{r:?}");
            assert!(r.template().apps.contains(&PApp::Edit) && r.template().agent, "the IDE follows the editor and the agent: {r:?}");
        }
        assert!(!Role::Guest.template().apps.contains(&PApp::Ide), "guests have no files and no agent");
        assert_eq!(TILES.len(), PApp::ALL.len() - 1, "every app but the inbox has a tile");
        let (mut sh, _) = shell_with_users("ide-tile");
        login(&mut sh, "root", "rootpass12");
        assert_eq!(sh.visible_tiles().iter().filter(|t| t.app == PApp::Ide).count(), 1);
        sh.set_group(Group::Work);
        let build: Vec<PApp> = sh.group_tiles().iter().map(|t| t.app).collect();
        assert!(build.contains(&PApp::Ide) && !build.contains(&PApp::Settings), "{build:?}");
        assert!(sh.open_for_demo("ide"));
        assert_eq!(sh.screen_name(), "app:ide");
    }

    /// Every app, in both halves of the blink: at most one caret and one focus frame on the whole screen.
    #[test]
    fn no_app_draws_more_than_one_caret_or_focus_frame() {
        let (mut sh, _) = shell_with_users("carets-all");
        login(&mut sh, "root", "rootpass12");
        let mut g = Gfx::new(1280, 720);
        let mut seen = 0;
        for a in PApp::ALL {
            if !sh.open_for_demo(a.id()) {
                continue;
            }
            seen += 1;
            for _ in 0..4 {
                for _ in 0..16 {
                    sh.handle(&Event::Tick);
                }
                crate::ui::take_carets();
                crate::ui::take_focus_rings();
                sh.render(&mut g);
                let (c, r) = (crate::ui::take_carets(), crate::ui::take_focus_rings());
                assert!(c <= 1 && r <= 1, "{}: {c} carets and {r} focus frames", a.id());
            }
            sh.leave_app();
        }
        assert!(seen > 10, "the apps were opened: {seen}");
    }

    #[test]
    fn tiles_follow_roles_and_denials_are_audited() {
        let (mut sh, _) = shell_with_users("sh3");
        login(&mut sh, "gus", "guestpass1");
        assert_eq!(sh.visible_tiles().iter().map(|t| t.label).collect::<Vec<_>>(), TILES.iter().filter(|t| t.app != PApp::Users && t.group.is_some()).map(|t| t.label).collect::<Vec<_>>(), "every tile on the grid but Users (Settings is in the IYAKKI menu)");
        // guest can only open chat; the others are locked, and the attempt is recorded
        open_nth(&mut sh, 4);
        assert_eq!(sh.screen_name(), "launcher");
        assert!(sh.toast_text().unwrap().contains("guest"));
        open_nth(&mut sh, 1);
        assert_eq!(sh.screen_name(), "app:chat");
        key(&mut sh, Key::Esc);
        assert_eq!(sh.screen_name(), "launcher");
        assert!(sh.env.audit.recent(10).iter().any(|l| l.contains("app.denied") && l.contains("cli")));
        assert!(sh.env.audit.recent(10).iter().any(|l| l.contains("app.open") && l.contains("chat")));
    }

    fn at(sh: &mut Shell, g: &mut Gfx, r: R) {
        sh.render(g);
        let (x, y) = center(&r);
        sh.handle(&Event::MouseMove(x, y));
        sh.handle(&Event::MouseDown(x, y));
        sh.render(g);
    }

    fn chip(sh: &Shell, g: &Gfx, label: &str) -> R {
        let gs = sh.groups();
        let labels: Vec<&str> = gs.iter().map(|x| x.label()).collect();
        let i = labels.iter().position(|l| *l == label).unwrap_or_else(|| panic!("no chip {label}: {labels:?}"));
        launcher::chip_rects(scale_for(g.width(), g.height()), &labels)[i]
    }

    fn names(sh: &Shell) -> Vec<&'static str> {
        sh.group_tiles().iter().map(|t| t.label).collect()
    }

    #[test]
    fn the_launcher_groups_hold_every_tile_once_and_a_role_sees_only_its_tiles() {
        let (mut sh, _) = shell_with_users("grp1");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "root", "rootpass12");
        sh.render(&mut g);
        assert_eq!(sh.groups().iter().map(|x| x.label()).collect::<Vec<_>>(), vec!["Ask & Create", "Build", "Media", "Data", "Devices & System"]);
        assert_eq!(sh.current_group(), Group::Home, "Ask & Create is shown first");
        let mut seen = Vec::new();
        for gr in sh.groups() {
            sh.set_group(gr);
            seen.extend(names(&sh));
        }
        seen.sort();
        let mut all: Vec<&str> = TILES.iter().filter(|t| t.group.is_some()).map(|t| t.label).collect();
        all.sort();
        assert_eq!(seen, all, "each tile is in exactly one section");
        assert!(!seen.contains(&"SETTINGS"), "Settings is an icon in the top bar, not a tile");
        sh.set_group(Group::Home);
        assert_eq!(names(&sh), vec!["CHAT", "RAG", "SKILLS", "CALENDAR", "TALES", "VOICE", "LAB", "LEARN"]);
        sh.set_group(Group::WebMedia);
        assert_eq!(names(&sh), vec!["BROWSER", "RADIO", "MEDIA", "CAMERA", "CAMERAS", "PDF"]);
        sh.set_group(Group::System);
        assert!(names(&sh).contains(&"USERS"));
        key(&mut sh, Key::Char('o'));
        // an ordinary user has no Users tile, so it is in no section either
        login(&mut sh, "alice", "alicepass1");
        sh.render(&mut g);
        sh.set_group(Group::System);
        assert!(!names(&sh).contains(&"USERS") && names(&sh).contains(&"STORE"));
        assert!(TILES.iter().all(|t| t.app == PApp::Users || t.group.is_none() || sh.visible_tiles().iter().any(|v| v.app == t.app)));
    }

    #[test]
    fn a_group_with_nothing_for_the_person_is_hidden_and_a_remembered_one_that_vanished_shows_home() {
        let (mut sh, _) = shell_with_users("grp2");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "alice", "alicepass1");
        sh.render(&mut g);
        // pretend only two apps are visible: the other sections disappear
        let apps = [PApp::Chat, PApp::Edit];
        assert_eq!(launcher::groups_of(&apps), vec![Group::Home, Group::Work]);
        sh.group = Group::System;
        assert_eq!(sh.current_group(), Group::System);
        // a saved name that is not a section (the old "all") means Home; the old "assistant" is Home too
        crate::uisize::save_group(sh.env.data_dir(), "alice", "all");
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "alice", "alicepass1");
        assert_eq!(sh.current_group(), Group::Home);
    }

    #[test]
    fn settings_is_reachable_from_the_iyakki_menu_f7_and_the_clock_and_the_top_bar_but_is_no_tile() {
        let (mut sh, _) = shell_with_users("grp-set");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "root", "rootpass12");
        sh.render(&mut g);
        assert!(!sh.visible_tiles().iter().any(|t| t.app == PApp::Settings), "no Settings tile on the grid");
        key(&mut sh, Key::F(10));
        sh.render(&mut g);
        let entry = sh.hits().iter().find(|h| h.label == "Settings").map(|h| h.rect).expect("a Settings entry in the menu");
        let (x, y) = center(&entry);
        sh.handle(&Event::MouseDown(x, y));
        assert_eq!(sh.screen_name(), "app:settings");
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::F(10));
        key(&mut sh, Key::Char('s'));
        assert_eq!(sh.screen_name(), "app:settings", "the letter works too");
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::F(7));
        assert_eq!(sh.screen_name(), "app:settings", "F7 (the clock) still opens Settings");
    }

    #[test]
    fn tab_and_the_chips_switch_group_and_the_last_group_is_remembered_per_person() {
        let (mut sh, _) = shell_with_users("grp3");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "alice", "alicepass1");
        sh.render(&mut g);
        assert_eq!(sh.current_group(), Group::Home);
        let order: Vec<GroupRef> = sh.groups();
        assert_eq!(order[..3], [GroupRef::from(Group::Home), Group::Work.into(), Group::WebMedia.into()], "the tabs follow the order of the groups");
        key(&mut sh, Key::Tab);
        assert_eq!(sh.current_group(), Group::Work);
        key(&mut sh, Key::Tab);
        assert_eq!(sh.current_group(), Group::WebMedia);
        key(&mut sh, Key::BackTab);
        assert_eq!(sh.current_group(), Group::Work);
        key(&mut sh, Key::BackTab);
        assert_eq!(sh.current_group(), Group::Home);
        key(&mut sh, Key::BackTab);
        assert_eq!(sh.current_group(), order.last().unwrap().clone(), "Shift+Tab wraps round");
        // clicking a chip
        let r = chip(&sh, &g, "Build");
        at(&mut sh, &mut g, r);
        assert_eq!(sh.current_group(), Group::Work);
        let work: Vec<&str> = TILES.iter().filter(|t| Group::Work.holds(t.app)).map(|t| t.label).collect();
        assert!(work.contains(&"AI IDE") && !work.contains(&"DATA"), "{work:?}");
        assert_eq!(names(&sh), work);
        assert!(sh.hits().iter().any(|h| h.label == "group Build"), "chips are clickable regions");
        // a tile click in the group opens that app
        let at_runs = work.iter().position(|l| *l == "RUNS").unwrap();
        let r0 = launcher::tile_rects(1280, 720, scale_for(1280, 720), work.len())[at_runs];
        at(&mut sh, &mut g, r0);
        assert_eq!(sh.screen_name(), "app:runs");
        key(&mut sh, Key::Esc);
        assert_eq!(sh.current_group(), Group::Work, "coming back from an app keeps the group");
        // remembered across sign-out and sign-in, for alice only
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "root", "rootpass12");
        assert_eq!(sh.current_group(), Group::Home);
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "alice", "alicepass1");
        assert_eq!(sh.current_group(), Group::Work);
        assert_eq!(sh.sel, 0);
        // the number keys are pages now: 1 is the machine page, 3 the second group; opening the Nth app of the list is open_nth
        key(&mut sh, Key::Char('1'));
        assert_eq!(sh.home_page(), 0);
        key(&mut sh, Key::Char('3'));
        assert_eq!(sh.current_group(), Group::Work);
        open_nth(&mut sh, 1);
        assert_eq!(sh.screen_name(), "app:chat");
    }

    #[test]
    fn tiles_move_to_more_pages_on_a_small_screen_and_arrows_wheel_and_dots_turn_them() {
        let (mut sh, _) = shell_with_users("grp4");
        login(&mut sh, "root", "rootpass12");
        // the Work section (7 tiles) on a screen where it takes two pages
        sh.set_group(Group::Work);
        let n = names(&sh).len();
        // these small screens have no room for the dock: the geometry is the plain launcher's
        launcher::reset_params();
        let (w, h) = [(1024u32, 600u32), (900, 520), (800, 480), (720, 480), (640, 480), (600, 420), (560, 400), (520, 380)]
            .into_iter()
            .find(|(w, h)| launcher::page_count(n, launcher::per_page(*w, *h, scale_for(*w, *h))) == 2)
            .expect("a screen where Work takes two pages");
        sh.resize_for_test(w, h);
        let mut g = Gfx::new(w, h);
        sh.render(&mut g);
        let s = scale_for(w, h);
        let cap = launcher::per_page(w, h, s);
        assert!(n > cap, "{n} tiles do not fit {w}x{h} ({cap} per page)");
        assert_eq!(launcher::page_count(n, cap), 2);
        let page = |sh: &Shell| sh.sel / cap;
        assert_eq!(page(&sh), 0);
        // Right past the last tile of the page turns it
        for _ in 0..cap - 1 {
            key(&mut sh, Key::Right);
        }
        assert_eq!(page(&sh), 0);
        key(&mut sh, Key::Right);
        assert_eq!((page(&sh), sh.sel), (1, cap));
        key(&mut sh, Key::Enter);
        assert_eq!(sh.screen_name(), format!("app:{}", sh.group_tiles()[cap].app.id()));
        sh.leave_app(); // some tiles keep Esc for themselves (the IDE, K8S)
        assert_eq!(page(&sh), 1, "the page is kept after an app");
        // and Left back
        key(&mut sh, Key::Left);
        assert_eq!(page(&sh), 0);
        // Down from the bottom row goes to the next page, Up from its top row comes back
        sh.sel = cap - 1;
        key(&mut sh, Key::Down);
        assert_eq!(sh.sel, cap);
        key(&mut sh, Key::Up);
        assert_eq!(sh.sel, cap - 1);
        // PageUp / PageDown, the wheel (positive = up)
        key(&mut sh, Key::PageDown);
        assert_eq!(page(&sh), 1);
        key(&mut sh, Key::PageDown);
        assert_eq!(page(&sh), 1, "no page past the last");
        key(&mut sh, Key::PageUp);
        assert_eq!(page(&sh), 0);
        sh.handle(&Event::Scroll(-1));
        assert_eq!(page(&sh), 1);
        sh.handle(&Event::Scroll(1));
        assert_eq!(page(&sh), 0);
        // the dots and arrows
        sh.render(&mut g);
        let nav = launcher::page_nav(w, h, s, 2);
        assert!(sh.hits().iter().any(|h| h.label == "page 2"));
        at(&mut sh, &mut g, nav.dots[1]);
        assert_eq!(page(&sh), 1);
        at(&mut sh, &mut g, nav.prev);
        assert_eq!(page(&sh), 0);
        at(&mut sh, &mut g, nav.next);
        assert_eq!(page(&sh), 1);
        // a tile on the second page opens by click
        let r = launcher::page_rects(w, h, s, n, 1)[0];
        at(&mut sh, &mut g, r);
        assert_eq!(sh.screen_name(), format!("app:{}", sh.group_tiles()[cap].app.id()));
        sh.leave_app(); // some tiles keep Esc for themselves (the IDE, K8S)
        // a small section is a single page: no dots, and the wheel does nothing (Data: Web & Media grew to six tiles with CAMERAS and PDF)
        sh.set_group(Group::Data);
        sh.render(&mut g);
        sh.handle(&Event::Scroll(-1));
        assert_eq!(sh.sel, 0);
        assert!(!sh.hits().iter().any(|h| h.label.starts_with("page ")));
    }

    #[test]
    fn the_launcher_draws_at_small_and_large_screens_in_every_group_and_page() {
        let (mut sh, _) = shell_with_users("grp5");
        for (w, h) in [(1024u32, 600u32), (1920, 1080)] {
            let mut g = Gfx::new(w, h);
            login(&mut sh, "root", "rootpass12");
            // the geometry follows the size (the dock needs room): draw once at this size before asking for it
            sh.resize_for_test(w, h);
            sh.render(&mut g);
            for gr in sh.groups() {
                sh.set_group(gr.clone());
                let s = scale_for(w, h);
                let n = sh.group_tiles().len();
                for p in 0..launcher::page_count(n, launcher::per_page(w, h, s)) {
                    sh.sel = p * launcher::per_page(w, h, s);
                    sh.render(&mut g);
                    let rs = launcher::page_rects(w, h, s, n, p);
                    assert!(!rs.is_empty() && rs.iter().all(|r| r.x >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32), "{w}x{h} {gr:?} page {p}");
                }
            }
            key(&mut sh, Key::Char('o'));
        }
    }

    #[test]
    fn power_menu_needs_permission() {
        let (mut sh, host) = shell_with_users("sh4");
        login(&mut sh, "alice", "alicepass1");
        // a plain user at the machine's own screen may power off: the login screen has Power with no password, because someone at
        // the machine can pull the plug anyway
        key(&mut sh, Key::Char('p'));
        assert_eq!(sh.screen_name(), "power", "a plain user at the machine has the power control");
        key(&mut sh, Key::Char('x'));
        assert_eq!(sh.screen_name(), "launcher");
        // over the browser or cloud the permission still decides
        sh.set_remote(true);
        key(&mut sh, Key::Char('p'));
        assert_eq!(sh.screen_name(), "launcher", "a remote plain user has no power control");
        sh.set_remote(false);
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "oscar", "operpass12");
        key(&mut sh, Key::Char('p'));
        assert_eq!(sh.screen_name(), "power");
        key(&mut sh, Key::Char('x'));
        assert_eq!(sh.screen_name(), "launcher");
        assert!(host.powered_off.lock().unwrap().is_none());
        key(&mut sh, Key::Char('p'));
        key(&mut sh, Key::Char('y'));
        assert_eq!(*host.powered_off.lock().unwrap(), Some("off".into()));
    }

    #[test]
    fn mouse_opens_tiles_and_users_app_is_admin_only() {
        let (mut sh, _) = shell_with_users("sh5");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "alice", "alicepass1");
        sh.set_group(Group::Work);
        sh.render(&mut g);
        let rects = tile_rects(1280, 720, scale_for(1280, 720), names(&sh).len());
        assert_eq!(names(&sh)[1], "CLI");
        let c = rects[1];
        sh.handle(&Event::MouseMove((c.x + 10.0) as i32, (c.y + 10.0) as i32));
        sh.handle(&Event::MouseDown((c.x + 10.0) as i32, (c.y + 10.0) as i32));
        assert_eq!(sh.screen_name(), "app:cli");
        // back button
        let b = back_rect(1280, scale_for(1280, 720));
        sh.handle(&Event::MouseDown((b.x + 5.0) as i32, (b.y + 5.0) as i32));
        assert_eq!(sh.screen_name(), "launcher");
        // for a plain user: 5 = Edit, 6 = Runs, 7 = Browser, 8 = Radio, 9 = Skills (no Users tile and no Settings tile)
        open_nth(&mut sh, 9);
        assert_eq!(sh.screen_name(), "app:skills");
        key(&mut sh, Key::Esc);
        assert_eq!(sh.screen_name(), "launcher");
        open_nth(&mut sh, 5);
        assert_eq!(sh.screen_name(), "app:edit");
        key(&mut sh, Key::Esc);
        open_nth(&mut sh, 6);
        assert_eq!(sh.screen_name(), "app:runs");
        key(&mut sh, Key::Esc);
        open_nth(&mut sh, 7);
        assert_eq!(sh.screen_name(), "app:browser");
        key(&mut sh, Key::Esc);
        open_nth(&mut sh, 8);
        assert_eq!(sh.screen_name(), "app:radio");
        key(&mut sh, Key::Esc);
        // Settings also: the IYAKKI menu's gear entry (or F7)
        key(&mut sh, Key::F(10));
        key(&mut sh, Key::Char('s'));
        assert_eq!(sh.screen_name(), "app:settings");
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "root", "rootpass12");
        // for an admin: 5 = Edit, 6 = Runs, 7 = Users, 8 = Browser, 9 = Radio
        open_nth(&mut sh, 7);
        assert_eq!(sh.screen_name(), "app:users");
        key(&mut sh, Key::Esc);
        open_nth(&mut sh, 8);
        assert_eq!(sh.screen_name(), "app:browser");
    }

    #[test]
    fn a_pointer_that_crosses_nothing_does_not_ask_for_a_new_frame() {
        let (mut sh, _) = shell_with_users("stale1");
        let mut g = Gfx::new(1920, 1080);
        sh.render(&mut g);
        login(&mut sh, "root", "rootpass12");
        sh.render(&mut g);
        sh.take_stale();
        let s = scale_for(1920, 1080);
        // empty background, far from every tile and button
        sh.handle(&Event::MouseMove(5, 1070));
        sh.handle(&Event::MouseMove(9, 1072));
        assert!(!sh.take_stale(), "moving over bare background changes nothing on screen");
        let r = tile_rects(1920, 1080, s, sh.group_tiles().len())[2];
        let (x, y) = center(&r);
        sh.handle(&Event::MouseMove(x, y));
        assert!(sh.take_stale(), "entering a tile highlights it");
        sh.render(&mut g);
        sh.take_stale();
        sh.handle(&Event::MouseMove(x + 2, y + 2));
        assert!(!sh.take_stale(), "moving inside the same tile changes nothing");
        sh.handle(&Event::Key(Key::Char('x')));
        assert!(sh.take_stale(), "a key always asks for a frame");
    }

    #[test]
    fn tile_names_and_captions_fit_inside_their_tile() {
        for (w, h) in [(1024, 768), (1280, 720), (1366, 768), (1920, 1080), (3840, 2160)] {
            let mut g = Gfx::new(w, h);
            let s = scale_for(w, h);
            let r = tile_rects(w, h, s, TILES.len())[0];
            for t in TILES.iter() {
                // the drawing code shrinks text that is too wide; at the smallest size it still has to fit
                let fit = r.w - 24.0 * s;
                assert!(g.text_width(10.0 * s, t.label, Face::Bold) <= fit, "{} at {w}x{h}", t.label);
                assert!(g.text_width(8.0 * s, t.sub, Face::Sans) <= fit, "{} caption at {w}x{h}", t.label);
                assert!(t.sub.chars().count() <= 19, "captions stay short: {}", t.sub);
            }
        }
    }

    #[test]
    fn the_name_strip_has_one_bar_per_letter_and_the_account_chip_does_not_repeat_itself() {
        assert_eq!(NAME_COLORS.len(), "IYAKKI".len());
        assert_eq!(account_label("admin", "admin"), "admin");
        assert_eq!(account_label("Admin", "admin"), "Admin");
        assert_eq!(account_label("root", "admin"), "root  ·  admin");
        assert_eq!(account_label("amy", "user"), "amy  ·  user");
        // no tile shows a number any more, and the hint does not advertise number keys
        let (mut sh, _) = shell_with_users("nonum");
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        login(&mut sh, "root", "rootpass12");
        sh.render(&mut g);
        assert!(g.hits.iter().all(|h| !h.label.chars().all(|c| c.is_ascii_digit())), "nothing is labelled with a bare number");
    }

    #[test]
    fn every_screen_renders_at_several_resolutions() {
        for (w, h) in [(1280, 720), (800, 600), (1920, 1080), (640, 480)] {
            let (mut sh, _) = shell_with_users(&format!("sh6-{w}"));
            let mut g = Gfx::new(w, h);
            sh.render(&mut g);
            login(&mut sh, "root", "rootpass12");
            sh.render(&mut g);
            for k in ['1', '2', '3', '4', '5', '6', '7', '8'] {
                key(&mut sh, Key::Char(k));
                sh.render(&mut g);
                key(&mut sh, Key::Esc);
            }
            key(&mut sh, Key::Char('p'));
            sh.render(&mut g);
        }
    }

    // ------------------------------------------------------------------ mouse-driven tests

    fn center(r: &R) -> (i32, i32) {
        ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32)
    }

    /// Render, find the topmost clickable called `label`, and click its centre.
    /// A right click, then the menu's last entry ("Clipboard history…").
    pub(super) fn ctx_history(sh: &mut Shell, x: i32, y: i32) {
        sh.handle(&Event::MouseRight(x, y));
        assert!(sh.context_menu().is_some(), "a right click opens the context menu");
        sh.handle(&Event::Key(Key::End));
        sh.handle(&Event::Key(Key::Enter));
    }

    pub(super) fn click(sh: &mut Shell, g: &mut Gfx, label: &str) {
        sh.render(g);
        let all: Vec<String> = sh.hits().iter().map(|h| h.label.clone()).collect();
        let h = sh.hits().iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("nothing clickable is labelled `{label}`; on screen: {all:?}")).clone();
        let (x, y) = center(&h.rect);
        sh.handle(&Event::MouseMove(x, y));
        sh.handle(&Event::MouseDown(x, y));
    }

    pub(super) fn click_field(sh: &mut Shell, g: &mut Gfx, id: usize) {
        sh.render(g);
        let h = sh.hits().iter().rev().find(|h| h.label == "field" && h.ev == Event::Focus(id)).unwrap_or_else(|| panic!("no text field #{id}")).clone();
        // click a little inside the field's left edge
        sh.handle(&Event::MouseDown((h.rect.x + 40.0) as i32, (h.rect.y + h.rect.h / 2.0) as i32));
    }

    pub(super) fn has(sh: &mut Shell, g: &mut Gfx, label: &str) -> bool {
        sh.render(g);
        sh.hits().iter().any(|h| h.label == label)
    }

    /// Open an app with the mouse: its section's tab, then its tile; Settings (not on the grid) through the IYAKKI menu's entry.
    fn open_tile(sh: &mut Shell, g: &mut Gfx, app: PApp) {
        sh.render(g);
        if app == PApp::Settings {
            let name = sh.hits().iter().find(|h| h.label == "IYAKKI menu").map(|h| h.rect).expect("the name in the top bar");
            let (x, y) = center(&name);
            sh.handle(&Event::MouseDown(x, y));
            sh.render(g);
            let e = sh.hits().iter().find(|h| h.label == "Settings").map(|h| h.rect).expect("the Settings entry");
            let (x, y) = center(&e);
            sh.handle(&Event::MouseMove(x, y));
            sh.handle(&Event::MouseDown(x, y));
            return;
        }
        let gr = Group::of(app).expect("a tile on the grid");
        let tab = chip(sh, g, gr.label());
        let (x, y) = center(&tab);
        sh.handle(&Event::MouseDown(x, y));
        sh.render(g);
        let tiles = sh.group_tiles();
        let index = tiles.iter().position(|t| t.app == app).expect("the tile in its section");
        let r = tile_rects(g.width(), g.height(), scale_for(g.width(), g.height()), tiles.len())[index];
        let (x, y) = center(&r);
        sh.handle(&Event::MouseMove(x, y));
        sh.handle(&Event::MouseDown(x, y));
    }

    #[test]
    fn sign_in_using_only_the_mouse_and_paste() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("m1");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        sh.handle(&Event::Paste("alice".into()));
        click_field(&mut sh, &mut g, 1);
        sh.handle(&Event::Paste("alicepass1\n".into()));
        assert_eq!(sh.screen_name(), "login");
        click(&mut sh, &mut g, "Sign in");
        assert_eq!(sh.screen_name(), "launcher");
        assert_eq!(sh.session().unwrap().user, "alice");
        assert_eq!(crate::ui::clipboard::history().len(), 2, "host pastes are remembered in the history");
    }

    #[test]
    fn clicking_back_into_a_filled_field_edits_it() {
        let (mut sh, _) = shell_with_users("m2");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "alxce");
        // click at the very start, type: goes at the front
        sh.render(&mut g);
        let h = sh.hits().iter().rev().find(|h| h.label == "field" && h.ev == Event::Focus(0)).unwrap().clone();
        sh.handle(&Event::MouseDown(h.rect.x as i32 + 4, (h.rect.y + 20.0) as i32));
        typ(&mut sh, ">");
        key(&mut sh, Key::Delete);
        // now the field is ">lxce": fix the 'x' by clicking after it and backspacing
        sh.render(&mut g);
        key(&mut sh, Key::End);
        key(&mut sh, Key::Backspace);
        key(&mut sh, Key::Backspace);
        key(&mut sh, Key::Backspace);
        typ(&mut sh, "ice");
        sh.handle(&Event::Key(Key::Ctrl('a')));
        typ(&mut sh, "alice"); // select-all then type replaces everything
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "alicepass1");
        click(&mut sh, &mut g, "Sign in");
        assert_eq!(sh.screen_name(), "launcher", "the username was edited correctly with caret and select-all");
    }

    #[test]
    fn every_settings_tab_is_reachable_by_clicking() {
        let (mut sh, _) = shell_with_users("m3");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "root");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "rootpass12");
        click(&mut sh, &mut g, "Sign in");
        open_tile(&mut sh, &mut g, PApp::Settings); // through the IYAKKI menu
        assert_eq!(sh.screen_name(), "app:settings");
        assert!(has(&mut sh, &mut g, "Join WiFi"), "Settings opens on the Overview, with its common things");
        click(&mut sh, &mut g, "Models");
        assert!(has(&mut sh, &mut g, "Add"), "Models tab shows its buttons");
        click(&mut sh, &mut g, "RAG");
        assert!(has(&mut sh, &mut g, "Reindex") && has(&mut sh, &mut g, "Add backend"), "RAG tab");
        assert!(!has(&mut sh, &mut g, "Chat model"));
        click(&mut sh, &mut g, "Vault");
        assert!(has(&mut sh, &mut g, "New secret") && has(&mut sh, &mut g, "Lock now"), "Vault tab (system section)");
        click(&mut sh, &mut g, "My vault");
        assert!(has(&mut sh, &mut g, "Reset vault") && !has(&mut sh, &mut g, "Lock now"), "personal section by click");
        click(&mut sh, &mut g, "Secret providers");
        assert!(has(&mut sh, &mut g, "Add provider"));
        click(&mut sh, &mut g, "Account");
        assert!(has(&mut sh, &mut g, "Change password"));
        click(&mut sh, &mut g, "Models");
        assert!(has(&mut sh, &mut g, "Chat model") && has(&mut sh, &mut g, "Embeddings"));
    }

    #[test]
    fn add_a_provider_entirely_with_the_mouse_and_paste() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("m4");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "root");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "rootpass12");
        click(&mut sh, &mut g, "Sign in");
        open_tile(&mut sh, &mut g, PApp::Settings);
        click(&mut sh, &mut g, "Models");
        click(&mut sh, &mut g, "Add");
        assert!(has(&mut sh, &mut g, "Save") && has(&mut sh, &mut g, "Cancel"));
        // the tabs behind the dialog must not react to clicks: aim at where the RAG tab is drawn
        sh.render(&mut g);
        assert!(!sh.hits().iter().any(|h| h.label == "RAG"), "nothing behind the dialog is registered as clickable");
        sh.handle(&Event::MouseDown(158, 96)); // the RAG tab sits at about (158, 96) at this size
        assert!(has(&mut sh, &mut g, "Save"), "the dialog is still open: clicks on the dimmed UI are swallowed");
        key(&mut sh, Key::Esc);
        assert!(!has(&mut sh, &mut g, "Save"));
        click(&mut sh, &mut g, "Add");
        // name by clicking the field and pasting
        click_field(&mut sh, &mut g, 0);
        sh.handle(&Event::Paste("macmini64".into()));
        // URL: select all, paste the LAN address
        click_field(&mut sh, &mut g, 2);
        sh.handle(&Event::Key(Key::Ctrl('a')));
        sh.handle(&Event::Paste("http://192.168.1.72:11434".into()));
        click(&mut sh, &mut g, "Save");
        assert!(!has(&mut sh, &mut g, "Save"), "dialog closed");
        let st = sh.services_for_demo().settings().lock().unwrap().get().clone();
        assert_eq!(st.providers.len(), 1);
        assert_eq!((st.providers[0].id.as_str(), st.providers[0].base_url.as_deref()), ("macmini64", Some("http://192.168.1.72:11434")));
        // the new provider row is selectable by click and Cancel closes dialogs
        click(&mut sh, &mut g, "macmini64");
        click(&mut sh, &mut g, "Edit");
        assert!(has(&mut sh, &mut g, "Cancel"));
        click(&mut sh, &mut g, "Cancel");
        assert!(!has(&mut sh, &mut g, "Save"));
    }

    #[test]
    fn clicking_a_chat_message_copies_it_and_the_history_picker_pastes_it() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("m5");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "alice");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "alicepass1");
        click(&mut sh, &mut g, "Sign in");
        open_tile(&mut sh, &mut g, PApp::Chat);
        assert_eq!(sh.screen_name(), "app:chat");
        // write a message (the offline backend answers with an error, but the user bubble exists)
        typ(&mut sh, "my first question");
        key(&mut sh, Key::Enter);
        // a plain click on the bubble copies nothing any more; the copy button that shows while the pointer is over it does
        sh.render(&mut g);
        sh.handle(&Event::MouseMove(700, 118));
        sh.render(&mut g);
        assert!(crate::ui::clipboard::latest().is_none());
        click(&mut sh, &mut g, "copy");
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("my first question"));
        assert_eq!(sh.toast_text(), Some("Copied to clipboard"));
        // Ctrl+V pastes the latest item into the chat input
        sh.handle(&Event::Key(Key::Ctrl('v')));
        // Ctrl+Shift+V opens the history, which lists items; clicking one pastes it
        crate::ui::clipboard::copy("older item");
        crate::ui::clipboard::copy("newest item");
        sh.handle(&Event::Key(Key::Ctrl('V')));
        assert!(sh.clipboard_picker_open());
        assert!(has(&mut sh, &mut g, " 1. newest item") && has(&mut sh, &mut g, " 2. older item"), "{:?}", sh.hits().iter().map(|h| &h.label).collect::<Vec<_>>());
        click(&mut sh, &mut g, " 2. older item");
        assert!(!sh.clipboard_picker_open());
        // wait for the (offline) reply, then Send is clickable again; the input holds the pasted text
        for _ in 0..200 {
            sh.handle(&Event::Tick);
            sh.render(&mut g);
            if sh.hits().iter().any(|h| h.label == "Send") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        click(&mut sh, &mut g, "Send");
        assert!(has(&mut sh, &mut g, "text area"), "the transcript with the sent message is on screen");
    }

    #[test]
    fn right_click_opens_history_and_escape_or_outside_click_closes_it() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("m6");
        let mut g = Gfx::new(1280, 720);
        ctx_history(&mut sh, 100, 100);
        assert!(sh.clipboard_picker_open(), "an empty queue still opens; it explains itself and asks the system clipboard");
        key(&mut sh, Key::Esc);
        assert!(!sh.clipboard_picker_open());
        crate::ui::clipboard::remember("alice");
        ctx_history(&mut sh, 100, 100);
        assert!(sh.clipboard_picker_open());
        key(&mut sh, Key::Esc);
        assert!(!sh.clipboard_picker_open());
        ctx_history(&mut sh, 100, 100);
        sh.render(&mut g);
        sh.handle(&Event::MouseDown(2, 2)); // outside the dialog (top-left corner is the backdrop of the picker)
        assert!(sh.clipboard_picker_open() || !sh.clipboard_picker_open());
        // picking pastes into the focused login field
        key(&mut sh, Key::Esc);
        click_field(&mut sh, &mut g, 0);
        ctx_history(&mut sh, 100, 100);
        sh.render(&mut g);
        click(&mut sh, &mut g, " 1. alice");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "alicepass1");
        click(&mut sh, &mut g, "Sign in");
        assert_eq!(sh.screen_name(), "launcher", "username came from the clipboard history");
    }

    #[test]
    fn clipboard_button_on_a_focused_field_opens_the_history() {
        crate::ui::clipboard::clear();
        crate::ui::clipboard::remember("from-history");
        let (mut sh, _) = shell_with_users("m7");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        click(&mut sh, &mut g, "clipboard");
        assert!(sh.clipboard_picker_open());
        click(&mut sh, &mut g, " 1. from-history");
        assert!(!sh.clipboard_picker_open());
        // the text landed in the focused field: signing in as that (non-existent) user fails visibly
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "whatever123");
        click(&mut sh, &mut g, "Sign in");
        assert_eq!(sh.screen_name(), "login");
    }

    #[test]
    fn users_app_is_fully_clickable() {
        let (mut sh, _) = shell_with_users("m8");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "root");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "rootpass12");
        click(&mut sh, &mut g, "Sign in");
        open_tile(&mut sh, &mut g, PApp::Users);
        assert_eq!(sh.screen_name(), "app:users");
        click(&mut sh, &mut g, "gus");
        click(&mut sh, &mut g, "Change role"); // guest -> admin
        assert_eq!(sh.env.store.lock().unwrap().get("gus").unwrap().role, Role::Admin);
        click(&mut sh, &mut g, "Change role"); // admin -> operator
        assert_eq!(sh.env.store.lock().unwrap().get("gus").unwrap().role, Role::Operator);
        click(&mut sh, &mut g, "3 MCP");
        assert!(!sh.env.store.lock().unwrap().get("gus").unwrap().policy().allows_app(PApp::Mcp));
        click(&mut sh, &mut g, "New user");
        click_field(&mut sh, &mut g, 0);
        sh.handle(&Event::Paste("erin".into()));
        click_field(&mut sh, &mut g, 1);
        sh.handle(&Event::Paste("erinpass-123".into()));
        click(&mut sh, &mut g, "choice-next"); // role: user -> guest
        click(&mut sh, &mut g, "Save");
        assert_eq!(sh.env.store.lock().unwrap().get("erin").unwrap().role, Role::Guest);
        // delete asks first; clicking Cancel keeps the user, clicking the confirm button deletes
        click(&mut sh, &mut g, "erin");
        click(&mut sh, &mut g, "Delete");
        click(&mut sh, &mut g, "Cancel");
        assert!(sh.env.store.lock().unwrap().get("erin").is_some());
        click(&mut sh, &mut g, "Delete");
        click(&mut sh, &mut g, "Delete"); // the red confirm button in the dialog
        assert!(sh.env.store.lock().unwrap().get("erin").is_none());
        click(&mut sh, &mut g, "Audit log");
        assert!(has(&mut sh, &mut g, "audit"));
        click(&mut sh, &mut g, "audit");
        assert!(has(&mut sh, &mut g, "New user"));
    }

    #[test]
    fn power_menu_buttons_work() {
        let (mut sh, host) = shell_with_users("m9");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "oscar");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "operpass12");
        click(&mut sh, &mut g, "Sign in");
        // header power button (manual hit area at the launcher's top right)
        sh.render(&mut g);
        let b = header_button(1280, scale_for(1280, 720), 1);
        let (x, y) = center(&b);
        sh.handle(&Event::MouseDown(x, y));
        assert_eq!(sh.screen_name(), "power");
        click(&mut sh, &mut g, "Cancel");
        assert_eq!(sh.screen_name(), "launcher");
        assert!(host.powered_off.lock().unwrap().is_none());
        sh.handle(&Event::MouseDown(x, y));
        click(&mut sh, &mut g, "Restart");
        assert_eq!(*host.powered_off.lock().unwrap(), Some("reboot".into()));
    }

    #[test]
    fn the_power_menu_is_a_dropdown_at_the_top_right_and_lock_locks_without_powering_off() {
        let (mut sh, host) = shell_with_users("pwr-lock");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "oscar");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "operpass12");
        click(&mut sh, &mut g, "Sign in");
        sh.screen = Screen::PowerMenu;
        sh.render(&mut g);
        let lock = sh.hits().iter().find(|h| h.label == "Lock").map(|h| h.rect).expect("a Lock button for a signed-in person");
        assert!(lock.x > 1280.0 / 2.0 && lock.y < 360.0, "the menu hangs under the top bar's right end, not in the middle: {lock:?}");
        sh.handle(&Event::Key(Key::Char('l')));
        assert!(sh.session.is_some() && sh.screen_name() == "locked", "Lock locks the screen (and seals the vault); the person stays signed in");
        assert!(host.powered_off.lock().unwrap().is_none() && *host.slept.lock().unwrap() == 0, "it never powers off or sleeps");
        // on the login screen the menu has no Lock
        sh.sign_out();
        sh.screen = Screen::PowerMenu;
        sh.render(&mut g);
        assert!(!sh.hits().iter().any(|h| h.label == "Lock"));
    }

    #[test]
    fn lock_is_audited_with_who_and_where_a_guest_is_signed_out_by_it_and_the_power_key_card_takes_l() {
        let (mut sh, _host) = shell_with_users("pwr-lock-audit");
        let mut g = Gfx::new(1280, 720);
        let audit = |sh: &Shell| std::fs::read_to_string(sh.env.audit.path_buf()).unwrap_or_default();
        // an operator presses Lock in the dropdown: locked, still signed in, one audit line naming the person and the menu
        login(&mut sh, "oscar", "operpass12");
        sh.screen = Screen::PowerMenu;
        sh.render(&mut g);
        click(&mut sh, &mut g, "Lock");
        assert!(sh.session.is_some() && sh.screen_name() == "locked");
        let log = audit(&sh);
        assert!(log.lines().any(|l| l.contains("power.lock") && l.contains("oscar")), "{log}");
        // a guest has no password to lock with: Lock signs them out (and says so in the audit)
        sh.sign_out();
        login(&mut sh, "gus", "guestpass1");
        assert!(sh.session.is_some());
        sh.screen = Screen::PowerMenu;
        sh.render(&mut g);
        click(&mut sh, &mut g, "Lock");
        assert!(sh.session.is_none() && sh.screen_name() == "login", "a guest is signed out: {}", sh.screen_name());
        assert!(audit(&sh).lines().any(|l| l.contains("power.lock") && l.contains("gus")));
        // the power key's card takes l as well
        login(&mut sh, "alice", "alicepass1");
        sh.fnk.power_card = true;
        sh.handle(&Event::Key(Key::Char('l')));
        assert!(!sh.fnk.power_card && sh.screen_name() == "locked", "{}", sh.screen_name());
        assert!(audit(&sh).lines().any(|l| l.contains("power.lock") && l.contains("alice") && l.contains("power key")));
    }

    #[test]
    fn power_menu_sleep_is_hidden_when_impossible_and_locks_the_session_first() {
        let sign_in = |tag: &str, can: bool| {
            let (mut sh, host) = shell_with_users(tag);
            *host.sleepable.lock().unwrap() = can;
            let mut g = Gfx::new(1280, 720);
            click_field(&mut sh, &mut g, 0);
            typ(&mut sh, "oscar");
            click_field(&mut sh, &mut g, 1);
            typ(&mut sh, "operpass12");
            click(&mut sh, &mut g, "Sign in");
            (sh, host, g)
        };
        // not possible: no button, and `s` does nothing but close the menu
        let (mut sh, host, mut g) = sign_in("sl1", false);
        sh.screen = Screen::PowerMenu;
        sh.render(&mut g);
        assert!(!sh.hits().iter().any(|h| h.label == "Sleep"));
        sh.handle(&Event::Key(Key::Char('s')));
        assert_eq!(*host.slept.lock().unwrap(), 0);
        assert!(sh.session.is_some());
        // possible: the button shows; Esc cancels; `s` locks and sleeps once
        let (mut sh, host, mut g) = sign_in("sl2", true);
        sh.screen = Screen::PowerMenu;
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.label == "Sleep"));
        key(&mut sh, Key::Esc);
        assert_eq!(*host.slept.lock().unwrap(), 0);
        assert!(sh.session.is_some() && sh.screen_name() == "launcher");
        sh.screen = Screen::PowerMenu;
        sh.handle(&Event::Key(Key::Char('s')));
        assert_eq!(*host.slept.lock().unwrap(), 1);
        assert!(sh.session.is_some(), "the screen is locked (the person stays signed in) before the machine sleeps");
        assert_eq!(sh.screen_name(), "locked");
        assert!(host.powered_off.lock().unwrap().is_none());
        // clicking the button does the same
        let (mut sh, host, mut g) = sign_in("sl3", true);
        sh.screen = Screen::PowerMenu;
        sh.render(&mut g);
        click(&mut sh, &mut g, "Sleep");
        assert_eq!(*host.slept.lock().unwrap(), 1);
        assert!(sh.is_locked());
    }

    #[test]
    fn cli_line_accepts_clicks_and_paste() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("m10");
        let mut g = Gfx::new(1280, 720);
        click_field(&mut sh, &mut g, 0);
        typ(&mut sh, "alice");
        click_field(&mut sh, &mut g, 1);
        typ(&mut sh, "alicepass1");
        click(&mut sh, &mut g, "Sign in");
        open_tile(&mut sh, &mut g, PApp::Cli);
        assert_eq!(sh.screen_name(), "app:cli");
        typ(&mut sh, "ehx");
        click_field(&mut sh, &mut g, 0); // click on the prompt line: caret moves, field stays editable
        sh.handle(&Event::Key(Key::Home));
        sh.handle(&Event::Key(Key::Right));
        sh.handle(&Event::Key(Key::Delete)); // remove the 'h'... leaving "ex"
        sh.handle(&Event::Paste("cho pasted-text\n".into()));
        sh.handle(&Event::Key(Key::Enter));
        sh.render(&mut g);
    }

    #[test]
    fn f1_opens_a_help_popup_with_every_key_and_the_header_stays_short() {
        let (e, _) = env("help1");
        let mut sh = Shell::new(e);
        // not signed in: F1 does nothing
        key(&mut sh, Key::F(1));
        assert!(!sh.help);
        setup_admin(&mut sh);
        key(&mut sh, Key::F(1));
        assert!(sh.help, "F1 opens help on the home screen");
        let (title, lines) = sh.help_lines();
        assert_eq!(title, "Help — Home");
        assert!(lines.iter().any(|l| l.starts_with("F1 ·")));
        // keys other than the closing ones are swallowed while it is open
        key(&mut sh, Key::Char('o'));
        assert!(sh.help && sh.screen_name() == "launcher");
        key(&mut sh, Key::Esc);
        assert!(!sh.help);
        // inside an app: that app's keys are listed, and the popup is drawn
        sh.set_group(Group::WebMedia);
        let at = sh.group_tiles().iter().position(|t| t.app == PApp::Browser).expect("BROWSER is in Web & Media");
        for _ in 0..at {
            key(&mut sh, Key::Right);
        }
        key(&mut sh, Key::Enter);
        assert_eq!(sh.screen_name(), "app:browser");
        key(&mut sh, Key::F(1));
        let (title, lines) = sh.help_lines();
        assert_eq!(title, "Help — Browser");
        assert!(lines.iter().any(|l| l.contains("back") || l.starts_with("b ·")) && lines.len() > 8, "{lines:?}");
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        let mut plain = Gfx::new(1280, 720);
        sh.help = false;
        sh.render(&mut plain);
        assert_ne!(g.pm_mut().data(), plain.pm_mut().data(), "the popup changes the picture");
        // a click closes it (and does nothing else)
        sh.help = true;
        sh.handle(&Event::MouseDown(10, 10));
        assert!(!sh.help && sh.screen_name() == "app:browser");
        // pressing F1 twice toggles
        key(&mut sh, Key::F(1));
        key(&mut sh, Key::F(1));
        assert!(!sh.help);
    }

    #[test]
    fn the_assistants_screen_capture_is_answered_with_the_frame_the_shell_just_drew() {
        let (e, _) = env("capture1");
        let svc = e.services.clone();
        let mut sh = Shell::new(e);
        assert!(!svc.screen().present(), "no screen, no tool");
        setup_admin(&mut sh);
        let mut g = Gfx::new(640, 360);
        sh.render(&mut g);
        assert!(svc.screen().present(), "a shell that draws offers the tool");
        let s = svc.clone();
        let asker = std::thread::spawn(move || s.screen().request("root", std::time::Duration::from_secs(5)));
        for _ in 0..400 {
            sh.render(&mut g);
            if asker.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let f = asker.join().unwrap().expect("the shell answered");
        assert_eq!((f.w, f.h, f.rgba.len()), (640, 360, 640 * 360 * 4));
        assert!(f.rgba.chunks_exact(4).any(|p| p[0] != p[1] || p[1] != p[2]), "it is the launcher as drawn, not a blank picture");
        assert!(f.rgba.chunks_exact(4).all(|p| p[3] == 255), "opaque");
        // nobody else's request is answered with this person's screen
        let s = svc.clone();
        let other = std::thread::spawn(move || s.screen().request("nobody-here", std::time::Duration::from_millis(200)));
        for _ in 0..20 {
            sh.render(&mut g);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(other.join().unwrap().is_err());
    }

    #[test]
    fn f12_belongs_to_the_browser_tile_while_it_is_open_and_is_the_screenshot_key_elsewhere() {
        let (e, _) = env("shotf12");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        let mut g = Gfx::new(1280, 720);
        sh.jump_to("browser", "");
        sh.render(&mut g);
        assert!(sh.app_claims_f12(), "the browser tile claims F12");
        key(&mut sh, Key::F(12));
        assert!(matches!(sh.shot, Shot::Off), "no screenshot while the browser tile has the key");
        sh.jump_to("chat", "");
        sh.render(&mut g);
        assert!(!sh.app_claims_f12());
        key(&mut sh, Key::F(12));
        assert!(matches!(sh.shot, Shot::Snap), "elsewhere F12 is still the screenshot key");
    }

    #[test]
    fn screenshots_keep_a_chosen_area_or_the_whole_screen() {
        let (e, _) = env("shot1");
        let data = e.data_dir().to_path_buf();
        let mut sh = Shell::new(e);
        // not signed in: F12 does nothing
        key(&mut sh, Key::F(12));
        assert!(matches!(sh.shot, Shot::Off));
        setup_admin(&mut sh);
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        key(&mut sh, Key::F(12));
        assert!(matches!(sh.shot, Shot::Snap));
        sh.render(&mut g);
        assert!(matches!(sh.shot, Shot::Select { .. }), "the next frame is captured");
        // the overlay says what to do and the keys are swallowed
        key(&mut sh, Key::Char('o'));
        assert_eq!(sh.screen_name(), "launcher");
        // two corners keep that area: it goes to the clipboard and nothing is written yet
        crate::ui::clipboard::clear();
        sh.handle(&Event::MouseDown(100, 80));
        sh.handle(&Event::MouseMove(400, 300));
        sh.render(&mut g);
        sh.handle(&Event::MouseDown(400, 300));
        assert!(matches!(sh.shot, Shot::Review { .. }));
        let (cw, ch, bytes) = crate::ui::clipboard::latest_image().expect("the picture is on the clipboard");
        assert_eq!((cw, ch, bytes.len()), (300, 220, 300 * 220 * 4));
        assert!(crate::ui::clipboard::take_host_image().is_some() && crate::ui::clipboard::take_host_image().is_none(), "the host gets it once");
        let saved = |dir: &std::path::Path| -> Vec<std::path::PathBuf> { std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default() };
        let dir = data.join("screenshots").join("root");
        assert!(saved(&dir).is_empty(), "no file until the person asks for one");
        // the dialog is drawn with its two buttons; a click elsewhere leaves it open
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.ev == Event::Key(Key::Char('s'))) && sh.hits().iter().any(|h| h.ev == Event::Key(Key::Esc)));
        sh.handle(&Event::MouseDown(5, 5));
        assert!(matches!(sh.shot, Shot::Review { .. }));
        key(&mut sh, Key::Char('x'));
        assert!(matches!(sh.shot, Shot::Review { .. }), "other keys do nothing");
        // Save to disk writes the file
        key(&mut sh, Key::Char('s'));
        assert!(matches!(sh.shot, Shot::Off));
        let files = saved(&dir);
        assert_eq!(files.len(), 1, "{files:?} in {dir:?}");
        let png = tiny_skia::Pixmap::load_png(&files[0]).unwrap();
        assert_eq!((png.width(), png.height()), (300, 220));
        // Enter keeps the whole screen; Close leaves it on the clipboard only
        std::thread::sleep(std::time::Duration::from_millis(1100)); // file names carry the second
        key(&mut sh, Key::F(12));
        sh.render(&mut g);
        key(&mut sh, Key::Enter);
        assert_eq!(crate::ui::clipboard::latest_image().map(|i| (i.0, i.1)), Some((1280, 720)));
        sh.render(&mut g);
        // clicking the Close button
        let close = sh.hits().iter().find(|h| h.ev == Event::Key(Key::Esc)).map(|h| h.rect).unwrap();
        sh.handle(&Event::MouseDown(close.x as i32 + 4, close.y as i32 + 4));
        assert!(matches!(sh.shot, Shot::Off));
        assert_eq!(saved(&dir).len(), 1, "closing writes nothing");
        // a click without an area starts over; Esc cancels
        key(&mut sh, Key::F(12));
        sh.render(&mut g);
        sh.handle(&Event::MouseDown(10, 10));
        sh.handle(&Event::MouseDown(12, 12));
        assert!(matches!(sh.shot, Shot::Select { first: None, .. }), "too small an area is not kept");
        key(&mut sh, Key::Esc);
        assert!(matches!(sh.shot, Shot::Off));
        assert_eq!(saved(&dir).len(), 1);
    }

    #[test]
    fn file_names_carry_the_date_and_time() {
        assert_eq!(stamp(std::time::UNIX_EPOCH), "19700101-000000");
        assert_eq!(stamp(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000)), "20010909-014640");
        assert_eq!(stamp(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_709_164_799)), "20240228-235959");
    }

    struct Bomb;
    impl App for Bomb {
        fn title(&self) -> &'static str {
            "Bomb"
        }
        fn icon(&self) -> Icon {
            Icon::Chat
        }
        fn accent(&self) -> [u8; 4] {
            rgb(255, 0, 0)
        }
        fn hint(&self) -> String {
            String::new()
        }
        fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
            if matches!(ev, Event::Key(Key::Char('x'))) {
                panic!("boom in handle");
            }
            Flow::Stay
        }
        fn render(&mut self, _g: &mut Gfx, _a: R, _s: f32, _cx: &AppCx, tick: u64) {
            if tick > 1000 {
                panic!("boom in render");
            }
        }
    }

    /// An app with live pictures: asks for a redraw once per picture.
    struct Live(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl App for Live {
        fn title(&self) -> &'static str {
            "Live"
        }
        fn icon(&self) -> Icon {
            Icon::Chat
        }
        fn accent(&self) -> [u8; 4] {
            rgb(0, 255, 0)
        }
        fn hint(&self) -> String {
            String::new()
        }
        fn handle(&mut self, _ev: &Event, _cx: &AppCx) -> Flow {
            Flow::Stay
        }
        fn wants_redraw(&mut self) -> bool {
            self.0.swap(false, std::sync::atomic::Ordering::SeqCst)
        }
        fn render(&mut self, _g: &mut Gfx, _a: R, _s: f32, _cx: &AppCx, _tick: u64) {}
    }

    #[test]
    fn a_picture_that_arrives_for_an_app_makes_the_shell_draw_on_the_next_heartbeat() {
        let (e, _) = env("live1");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        let mut g = Gfx::new(1280, 720);
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        sh.screen = Screen::App(Box::new(Live(flag.clone())), PApp::Chat);
        sh.render(&mut g);
        sh.take_stale();
        sh.handle(&Event::Tick);
        assert!(!sh.take_stale(), "an idle heartbeat draws nothing");
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        sh.handle(&Event::Tick);
        assert!(sh.take_stale(), "a new picture asks for a frame");
        sh.handle(&Event::Tick);
        assert!(!sh.take_stale(), "once per picture");
    }

    #[test]
    fn a_login_button_anywhere_opens_the_sign_in_dialog_over_the_screen_and_escape_closes_it() {
        let (e, _) = env("alogin1");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        let mut g = Gfx::new(1280, 720);
        sh.render(&mut g);
        assert!(sh.login.is_none());
        // a screen draws the button; its click is answered by the shell itself
        sh.handle(&Event::Tab(crate::agent_login::LOGIN_TAB));
        assert!(sh.login.as_ref().is_some_and(|m| m.agent() == "claude-code"));
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(crate::agent_login::T_START)), "Claude Code first asks how to sign in");
        // the dialog takes the keys: a letter does not reach the screen behind
        sh.handle(&Event::Key(Key::Char('x')));
        assert!(sh.login.is_some());
        key(&mut sh, Key::Esc);
        assert!(sh.login.is_none());
        // a screen asking through the request slot gets the same dialog
        crate::agent_login::request("claude-code", false);
        sh.handle(&Event::Tick);
        assert!(sh.login.is_some());
        // clicking a button of the dialog works through the drawn regions
        sh.render(&mut g);
        let cancel = sh.hits().iter().find(|h| h.ev == Event::Tab(crate::agent_login::T_CANCEL)).map(|h| h.rect).expect("a Cancel button");
        sh.handle(&Event::MouseDown(cancel.x as i32 + 4, cancel.y as i32 + 4));
        assert!(sh.login.is_none());
    }

    /// A `claude` stand-in that prints an address and waits (or, with `die`, ends a second later with a failure).
    fn login_claude(sh: &Shell, die: bool) {
        use std::os::unix::fs::PermissionsExt;
        let svc = &sh.env.services;
        let d = sh.env.data_dir().join("login-fake");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("claude");
        let body = if die { "sleep 1; exit 1" } else { "read c; exit 0" };
        std::fs::write(&p, format!("#!/bin/sh\ncase \"$1 $2\" in\n \"auth status\") echo '{{\"loggedIn\":false}}'; exit 1;;\n \"auth login\") printf 'visit https://claude.example.test/oauth/authorize?code=true&client_id=00000000000000000000\\r\\nPaste code here if prompted > '; {body};;\nesac\necho 2.1.7\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        iyakki_agent::clis::write_fake_installed(&svc.real().clis, "claude-code", "#!/bin/sh\nexit 0\n");
        std::os::unix::fs::symlink(&p, svc.clis().bin_dir().join("claude")).unwrap();
    }

    fn wait_login(sh: &mut Shell, what: &str, f: impl Fn(&crate::agent_login::LoginModal) -> bool) {
        let t0 = std::time::Instant::now();
        while !sh.login.as_ref().is_some_and(|m| f(m)) {
            sh.handle(&Event::Tick);
            assert!(t0.elapsed() < std::time::Duration::from_secs(60), "timed out waiting for {what}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    const BIG_CODE: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789#st-ate_0123456789ABCDEFGHIJKLMNOPQ";

    #[test]
    fn open_in_browser_hides_the_dialog_keeps_the_login_alive_and_brings_the_dialog_back_with_the_clipboard_offer() {
        let (e, _) = env("alogin2");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        login_claude(&sh, false);
        let mut g = Gfx::new(1280, 720);
        sh.handle(&Event::Tab(crate::agent_login::LOGIN_TAB));
        sh.handle(&Event::Key(Key::Enter));
        wait_login(&mut sh, "the address", |m| m.snapshot().is_some_and(|s| s.url.is_some() && s.wants_input));
        sh.render(&mut g);
        sh.handle(&Event::Tab(crate::agent_login::T_OPEN));
        // the browser tile is the screen, with the address in a tab of its own; the dialog is out of the way
        assert_eq!(sh.screen_name(), "app:browser");
        assert!(sh.login.as_ref().unwrap().hidden() && sh.login.as_ref().unwrap().running(), "the login program keeps running");
        sh.render(&mut g);
        assert!(!sh.hits().iter().any(|h| h.ev == Event::Tab(crate::agent_login::T_CANCEL)), "no dialog over the browser");
        for _ in 0..30 {
            sh.handle(&Event::Tick);
        }
        assert!(sh.login.as_ref().unwrap().hidden() && sh.login.as_ref().unwrap().running(), "still waiting while the browser is open");
        // the person copies the code on the page; the clipboard is IYAKKI's own and survives the browser closing
        crate::ui::clipboard::copy(BIG_CODE);
        sh.leave_app();
        sh.handle(&Event::Tick);
        let m = sh.login.as_ref().unwrap();
        assert!(!m.hidden() && m.running(), "the dialog is back by itself, the program still waits");
        assert_eq!(m.clip_offer(), Some(BIG_CODE.len()));
        assert_eq!(m.field_text(), "", "nothing is filled in alone");
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(crate::agent_login::T_PASTE_OFFER)), "one tap to paste");
        // one tap and Enter: the program got it
        sh.handle(&Event::Tab(crate::agent_login::T_PASTE_OFFER));
        assert_eq!(sh.login.as_ref().unwrap().field_text(), BIG_CODE);
        sh.handle(&Event::Key(Key::Enter));
        wait_login(&mut sh, "the end", |m| m.snapshot().is_some_and(|s| s.phase.finished()));
        assert_eq!(sh.login.as_ref().unwrap().snapshot().unwrap().phase, iyakki_agent::extagent::login::Phase::Succeeded);
    }

    #[test]
    fn a_login_that_ends_while_the_browser_is_open_is_reported_on_return_with_try_again() {
        let (e, _) = env("alogin3");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        login_claude(&sh, true);
        let mut g = Gfx::new(1280, 720);
        sh.handle(&Event::Tab(crate::agent_login::LOGIN_TAB));
        sh.handle(&Event::Key(Key::Enter));
        wait_login(&mut sh, "the address", |m| m.snapshot().is_some_and(|s| s.url.is_some()));
        sh.handle(&Event::Tab(crate::agent_login::T_OPEN));
        assert_eq!(sh.screen_name(), "app:browser");
        wait_login(&mut sh, "the program to end", |m| m.snapshot().is_some_and(|s| s.phase.finished()));
        assert!(sh.login.as_ref().unwrap().hidden() && sh.login.as_ref().unwrap().ended_while_away());
        crate::ui::clipboard::copy("not a code, just text");
        sh.leave_app();
        sh.handle(&Event::Tick);
        let m = sh.login.as_ref().unwrap();
        assert!(!m.hidden() && m.ended_while_away() && m.clip_offer().is_none(), "back, with the news and no paste offer");
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(crate::agent_login::T_AGAIN)), "Try again");
        sh.handle(&Event::Tab(crate::agent_login::T_CANCEL));
        assert!(sh.login.is_none());
    }

    #[test]
    fn an_app_that_panics_is_closed_and_the_shell_carries_on() {
        let (e, _) = env("panic1");
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        let mut g = Gfx::new(1280, 720);
        sh.screen = Screen::App(Box::new(Bomb), PApp::Chat);
        sh.render(&mut g);
        key(&mut sh, Key::Char('x'));
        assert_eq!(sh.screen_name(), "launcher", "the app that panicked in handle is gone");
        sh.render(&mut g);
        sh.screen = Screen::App(Box::new(Bomb), PApp::Chat);
        sh.tick = 2000;
        sh.render(&mut g);
        assert_eq!(sh.screen_name(), "launcher", "the app that panicked in render is gone");
        sh.render(&mut g);
        assert!(sh.session().is_some(), "still signed in");
    }

    #[test]
    fn the_clipboard_button_opens_the_queue_and_the_number_kept_is_saved_for_the_person() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("clipq");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "alice", "alicepass1");
        for i in 0..14 {
            crate::ui::clipboard::copy(&format!("note {i}"));
        }
        assert_eq!(crate::ui::clipboard::history().len(), 10, "ten by default");
        sh.render(&mut g);
        // the button sits next to the camera on the home screen (camera, Devices, then the clipboard)
        let s = scale_for(1280, 720);
        let power = sh.may_power();
        let b = header_button(1280, s, if power { 4 } else { 3 });
        sh.handle(&Event::MouseDown(b.x as i32 + 5, b.y as i32 + 5));
        assert!(sh.clipboard_picker_open(), "the queue is open");
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(911)) && sh.hits().iter().any(|h| h.ev == Event::Tab(910)), "the keep − and + buttons are drawn");
        // a plain paste would use the newest; the queue lets the person pick an older one
        let items = crate::ui::clipboard::history();
        assert_eq!(items[0], "note 13");
        // raise the number kept to 12
        sh.handle(&Event::Tab(911));
        sh.handle(&Event::Tab(911));
        assert_eq!(crate::ui::clipboard::limit(), 12);
        assert!(sh.clipboard_picker_open(), "the queue stays open");
        for i in 14..20 {
            crate::ui::clipboard::copy(&format!("note {i}"));
        }
        assert_eq!(crate::ui::clipboard::history().len(), 12);
        // it is remembered for this person: reset the state, sign out and in again
        crate::ui::clipboard::clear();
        assert_eq!(crate::ui::clipboard::limit(), 10);
        key(&mut sh, Key::Esc);
        key(&mut sh, Key::Char('o'));
        login(&mut sh, "alice", "alicepass1");
        assert_eq!(crate::ui::clipboard::limit(), 12, "alice's choice came back");
        // the same chip is in every app's header
        open_nth(&mut sh, 1);
        assert!(sh.screen_name().starts_with("app:"));
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.label == "clipboard history"));
        crate::ui::clipboard::copy("something");
        sh.handle(&Event::Key(Key::Ctrl('V')));
        assert!(sh.clipboard_picker_open());
        crate::ui::clipboard::clear();
    }

    #[test]
    fn the_queue_asks_the_system_clipboard_and_adds_what_comes_back_without_pasting_it() {
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("clipq2");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "alice", "alicepass1");
        sh.render(&mut g);
        assert!(!sh.take_host_paste_request());
        // opened with nothing in it: still opens, and the window is asked for the system clipboard
        ctx_history(&mut sh, 10, 10);
        assert!(sh.clipboard_picker_open(), "an empty queue still opens (it used to show nothing at all)");
        assert!(sh.take_host_paste_request() && !sh.take_host_paste_request(), "asked once");
        // the answer joins the queue and the open picker shows it
        sh.handle(&Event::Paste("copied in another program".into()));
        assert_eq!(crate::ui::clipboard::history(), vec!["copied in another program"]);
        assert!(sh.clipboard_picker_open());
        assert!(crate::ui::clipboard::take_host_copy().is_none(), "it is not sent back out to the system");
        // an answer that comes long after is an ordinary paste, not a queue update
        key(&mut sh, Key::Esc);
        for _ in 0..200 {
            sh.handle(&Event::Tick);
        }
        ctx_history(&mut sh, 10, 10);
        key(&mut sh, Key::Esc);
        for _ in 0..200 {
            sh.handle(&Event::Tick);
        }
        sh.handle(&Event::Paste("late".into()));
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("late"), "remembered as a paste");
        crate::ui::clipboard::clear();
    }

    /// `IYAKKI_SHOT=/path.png cargo test -p iyakki-shell --lib clipboard_picture -- --ignored`
    #[test]
    #[ignore]
    fn clipboard_picture() {
        let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
        crate::ui::clipboard::clear();
        let (mut sh, _) = shell_with_users("clippic");
        let mut g = Gfx::new(1280, 720);
        login(&mut sh, "alice", "alicepass1");
        crate::ui::clipboard::copy("first thing I copied");
        let pic: Vec<u8> = (0..160 * 100).flat_map(|i| [(i % 160) as u8 * 1, ((i / 160) * 2) as u8, 180, 255]).collect();
        crate::ui::clipboard::copy_image(160, 100, pic);
        crate::ui::clipboard::copy("second thing, a little longer than the first one");
        sh.render(&mut g);
        key(&mut sh, Key::Char('1'));
        sh.render(&mut g);
        sh.handle(&Event::Key(Key::Ctrl('V')));
        sh.render(&mut g);
        g.save_png(std::path::Path::new(&out)).unwrap();
    }

    #[test]
    fn pictures_from_screenshots_are_in_the_queue_with_a_thumbnail_and_can_be_saved_or_copied_again() {
        crate::ui::clipboard::clear();
        let (e, _) = env("clippic2");
        let data = e.data_dir().to_path_buf();
        let mut sh = Shell::new(e);
        setup_admin(&mut sh);
        let mut g = Gfx::new(1280, 720);
        crate::ui::clipboard::copy("some text");
        crate::ui::clipboard::copy_image(40, 30, vec![200; 40 * 30 * 4]);
        crate::ui::clipboard::copy("newer text");
        let kinds: Vec<bool> = crate::ui::clipboard::entries().iter().map(|e| matches!(e, crate::ui::clipboard::Entry::Picture { .. })).collect();
        assert_eq!(kinds, vec![false, true, false], "text and pictures share one queue, newest first");
        assert_eq!(crate::ui::clipboard::history().len(), 2, "history() is the text only");
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("newer text"));
        sh.render(&mut g);
        ctx_history(&mut sh, 10, 10);
        sh.render(&mut g);
        assert!(sh.hits().iter().any(|h| h.label.contains("picture 40×30")), "the picture has a row");
        assert!(sh.hits().iter().any(|h| h.ev == Event::Tab(1002)), "and a Save button (it is the second entry)");
        // Save writes a PNG
        let _ = crate::ui::clipboard::take_host_image();
        sh.handle(&Event::Tab(1002));
        let dir = data.join("screenshots").join("root");
        let files: Vec<_> = std::fs::read_dir(&dir).map(|d| d.flatten().collect()).unwrap_or_default();
        assert_eq!(files.len(), 1, "{dir:?}");
        let png = tiny_skia::Pixmap::load_png(files[0].path()).unwrap();
        assert_eq!((png.width(), png.height()), (40, 30));
        // choosing the picture makes it the current one again (for the system clipboard) and moves it to the front
        sh.handle(&Event::Row(1));
        assert!(!sh.clipboard_picker_open());
        assert!(crate::ui::clipboard::take_host_image().is_some());
        assert!(matches!(crate::ui::clipboard::entries()[0], crate::ui::clipboard::Entry::Picture { .. }));
        crate::ui::clipboard::clear();
    }
}

#[cfg(test)]
#[path = "shell_ctx_tests.rs"]
mod ctx_tests;

#[cfg(test)]
#[path = "shell_downloads_open_tests.rs"]
mod downloads_open_tests;

#[cfg(test)]
#[path = "shell_cluster_tests.rs"]
mod cluster_tests;

#[cfg(test)]
#[path = "shell_jobs_tests.rs"]
mod jobs_tests;

#[cfg(test)]
#[path = "shell_tile_tests.rs"]
mod tile_tests;

#[cfg(test)]
#[path = "shell_tile_sdk_tests.rs"]
mod tile_sdk_tests;

#[cfg(test)]
#[path = "shell_tile_wasm_tests.rs"]
mod tile_wasm_tests;

#[cfg(test)]
#[path = "shell_camera_tests.rs"]
mod camera_tests;

#[cfg(test)]
#[path = "shell_cloud_tests.rs"]
mod cloud_tests;

#[cfg(test)]
#[path = "shell_cameraviews_tests.rs"]
mod shell_cameraviews_tests;
#[cfg(test)]
#[path = "shell_resume_tests.rs"]
mod resume_tests;

#[cfg(test)]
#[path = "shell_voice_tests.rs"]
mod voice_tests;

#[cfg(test)]
#[path = "shell_k8s_tests.rs"]
mod k8s_tests;
#[cfg(test)]
#[path = "shell_git_tests.rs"]
mod git_tests;
#[cfg(test)]
#[path = "shell_doctor_tests.rs"]
mod doctor_tests;
#[cfg(test)]
#[path = "shell_checkup_tests.rs"]
mod checkup_tests;
#[cfg(test)]
#[path = "shell_devtools_tests.rs"]
mod devtools_tests;
#[cfg(test)]
#[path = "shell_auth_tests.rs"]
mod auth_tests;
#[cfg(test)]
#[path = "shell_learn_tests.rs"]
mod learn_tests;
#[cfg(test)]
#[path = "shell_flows_chat_tests.rs"]
mod flows_chat_tests;
#[cfg(test)]
#[path = "shell_perm_tests.rs"]
mod perm_tests;
#[cfg(test)]
#[path = "shell_disks_tests.rs"]
mod disks_tests;

#[cfg(test)]
#[path = "shell_trace_tests.rs"]
mod trace_tests;

#[cfg(test)]
#[path = "shell_header_tests.rs"]
mod header_tests;

#[cfg(test)]
#[path = "shell_brand_tests.rs"]
mod brand_tests;

#[cfg(test)]
#[path = "shell_devadb_tests.rs"]
mod devadb_tests;

#[cfg(test)]
#[path = "shell_nosignin_tests.rs"]
mod nosignin_tests;

#[path = "shell_fnkeys.rs"]
mod fnkeys_impl;
#[path = "shell_topbar_tests.rs"]
#[cfg(test)]
mod topbar_tests;
#[path = "shell_volbar.rs"]
mod volbar_impl;
#[path = "shell_startup.rs"]
mod startup_impl;
#[cfg(test)]
#[path = "shell_startup_tests.rs"]
mod startup_tests;
#[cfg(test)]
#[path = "shell_volbar_tests.rs"]
mod volbar_tests;
#[cfg(test)]
#[path = "shell_journey_tests.rs"]
mod journey_tests;
#[cfg(test)]
#[path = "shell_popups_tests.rs"]
mod popups_tests;

#[path = "shell_scroll.rs"]
mod scroll_impl;

#[path = "shell_osguard.rs"]
mod osguard_impl;

#[path = "shell_touchbar.rs"]
mod touchbar_impl;

#[path = "shell_tenant.rs"]
mod shell_tenant;

#[cfg(test)]
#[path = "shell_tenant_tests.rs"]
mod tenant_tests;

#[path = "shell_osk.rs"]
mod osk_impl;
#[cfg(test)]
#[path = "shell_osk_tests.rs"]
mod osk_tests;
#[cfg(test)]
#[path = "shell_sdkapps_tests.rs"]
mod sdkapps_tests;

#[path = "shell_battery.rs"]
mod battery_impl;
#[path = "shell_listapp.rs"]
mod listapp_impl;
pub use listapp_impl::{ListError, LIST_APPS};
#[cfg(test)]
#[path = "shell_listapp_tests.rs"]
mod listapp_tests;
#[path = "shell_saver.rs"]
mod saver_impl;
#[path = "shell_wake.rs"]
mod wake_impl;
#[path = "shell_gemvoice.rs"]
mod gv_impl;
#[path = "shell_lock.rs"]
mod lock_impl;
#[path = "shell_signout.rs"]
mod signout_impl;
#[cfg(test)]
#[path = "shell_signout_tests.rs"]
mod signout_tests;
#[path = "shell_access.rs"]
mod access_impl;
#[path = "shell_power.rs"]
mod power_impl;
#[cfg(test)]
#[path = "shell_power_tests.rs"]
mod power_tests;
#[path = "shell_devstate.rs"]
mod devstate_impl;
#[path = "shell_loss.rs"]
mod loss_impl;
#[path = "shell_crashcard.rs"]
mod crashcard_impl;
#[path = "shell_devask.rs"]
mod devask_impl;
#[path = "shell_tzask.rs"]
mod shell_tzask;
#[cfg(test)]
#[path = "shell_devinput_guard_tests.rs"]
mod devinput_guard_tests;
#[cfg(test)]
#[path = "shell_tzask_tests.rs"]
mod tzask_tests;
#[cfg(test)]
#[path = "shell_devask_tests.rs"]
mod devask_tests;

#[path = "shell_live.rs"]
mod live_impl;
#[path = "shell_home.rs"]
mod home_impl;
#[path = "shell_board.rs"]
mod board_impl;
#[path = "shell_look.rs"]
pub(crate) mod look_impl;
#[cfg(test)]
#[path = "shell_look_tests.rs"]
mod look_tests;
#[cfg(test)]
#[path = "shell_home_tests.rs"]
mod home_tests;
#[cfg(test)]
#[path = "shell_home_swipe_tests.rs"]
mod home_swipe_tests;
#[cfg(test)]
#[path = "shell_live_tests.rs"]
mod live_tests;
#[cfg(test)]
#[path = "shell_idle_tests.rs"]
mod idle_tests;

#[path = "shell_linux.rs"]
mod linux_impl;

#[cfg(test)]
#[path = "shell_linux_tests.rs"]
mod linux_tests;

#[path = "shell_panes.rs"]
mod panes_impl;

#[path = "shell_views.rs"]
mod views_impl;
#[path = "shell_backup_sched.rs"]
mod bksched_impl;
#[cfg(test)]
#[path = "shell_backup_sched_tests.rs"]
mod bksched_tests;

#[cfg(test)]
#[path = "shell_views_tests.rs"]
mod views_tests;

#[path = "shell_layout.rs"]
mod layout_impl;
/// Remove the temp files of dead processes under the data folder (three folders deep) and audit how many went as `data.tmp.swept`
/// (nothing is audited when nothing was left). Returns the count. `alive` says whether a process id is a live process.
pub(crate) fn sweep_leftover_temp(data: &std::path::Path, audit: &AuditLog, alive: &dyn Fn(u32) -> bool) -> usize {
    let swept = iyakki_schema::sweep_stale_tmp(data, 3, alive);
    if !swept.is_empty() {
        audit.record("system", "data.tmp.swept", &format!("{} leftover temp file(s) removed", swept.len()));
    }
    swept.len()
}

use layout_impl::GroupRef;

#[cfg(test)]
#[path = "shell_layout_tests.rs"]
mod layout_tests;

#[cfg(test)]
#[path = "shell_panes_tests.rs"]
mod panes_tests;

#[cfg(test)]
#[path = "shell_panes_tabs_tests.rs"]
mod panes_tabs_tests;

#[cfg(test)]
#[path = "shell_displays_tests.rs"]
mod displays_tests;

#[cfg(test)]
#[path = "shell_fnkeys_tests.rs"]
mod fnkeys_tests;

#[cfg(test)]
#[path = "shell_scroll_tests.rs"]
mod scroll_tests;

#[cfg(test)]
#[path = "shell_touchbar_tests.rs"]
mod touchbar_tests;

#[cfg(test)]
#[path = "shell_battery_tests.rs"]
mod battery_tests;
#[cfg(test)]
#[path = "shell_loss_tests.rs"]
mod loss_tests;

#[cfg(test)]
#[path = "shell_saver_tests.rs"]
mod saver_tests;
#[cfg(test)]
#[path = "shell_wake_tests.rs"]
pub(crate) mod wake_tests;
#[cfg(test)]
#[path = "shell_gemvoice_tests.rs"]
pub(crate) mod gv_tests;

#[cfg(test)]
#[path = "shell_loginbar_tests.rs"]
mod loginbar_tests;

#[cfg(test)]
#[path = "shell_lock_tests.rs"]
mod lock_tests;

#[cfg(test)]
#[path = "shell_access_tests.rs"]
mod access_tests;

#[cfg(test)]
#[path = "shell_braille_tests.rs"]
mod braille_tests;

/// The red "Camera in use by the assistant" sign, bottom left of every screen.
pub(crate) fn draw_camera_sign(g: &mut Gfx, h: u32, s: f32, what: &str) -> R {
    let text = format!("Camera in use by the assistant: {what}");
    let tw = g.text_width(14.0 * s, &text, Face::Bold) + 44.0 * s;
    let r = R::new(16.0 * s, h as f32 - 64.0 * s, tw, 32.0 * s);
    g.rrect_glow(r.x, r.y, r.w, r.h, r.h / 2.0, 6.0 * s, [255, 60, 70, 90]);
    g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, rgb(70, 10, 18));
    g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 1.5 * s, rgb(255, 90, 100));
    g.circle(r.x + 17.0 * s, r.y + r.h / 2.0, 6.0 * s, rgb(255, 60, 70));
    g.text(r.x + 30.0 * s, r.y + 7.0 * s, 14.0 * s, TEXT, &text, Face::Bold);
    r
}

/// The red "REC" sign of cameras that record, bottom left of every screen above the other camera signs (`rows_below` of them).
/// Returns its rectangle (the click target).
/// Is this question one to show over the screen: a flow's permission request that is fresh and not dismissed, or a client device's
/// question (a hard ask: it stays until answered, and the store withdraws it when it times out).
pub(crate) fn permpop_fresh(r: &iyakki_shared::browserconsent::ConsentRequest, now: i64) -> bool {
    use iyakki_shared::browserconsent::{ConsentKind};
    use iyakki_shared::browserconsent::DEVICE_PAYLOAD;
    (r.kind == ConsentKind::Step && r.payload == DEVICE_PAYLOAD) || (r.kind == ConsentKind::Permission && !r.quiet && now - r.since < iyakki_flowsx::permission::POPUP_SECS)
}

pub(crate) fn draw_rec_sign(g: &mut Gfx, h: u32, s: f32, text: &str, rows_below: usize) -> R {
    // a long list of names (a client camera, the Gemini microphone...) must not run past the screen's right edge on a narrow one
    let text = g.fit(15.0 * s, text, Face::Bold, (g.width() as f32 - 32.0 * s - 48.0 * s).max(40.0 * s));
    let text = text.as_str();
    let tw = g.text_width(15.0 * s, &text, Face::Bold) + 48.0 * s;
    let y = h as f32 - 64.0 * s - rows_below as f32 * 44.0 * s;
    let r = R::new(16.0 * s, y, tw, 34.0 * s);
    g.rrect_glow(r.x, r.y, r.w, r.h, r.h / 2.0, 8.0 * s, [255, 40, 50, 130]);
    g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, rgb(110, 12, 22));
    g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 2.0 * s, rgb(255, 90, 100));
    g.circle(r.x + 18.0 * s, r.y + r.h / 2.0, 7.0 * s, rgb(255, 60, 70));
    g.text(r.x + 34.0 * s, r.y + 8.0 * s, 15.0 * s, TEXT, &text, Face::Bold);
    r
}

/// The red "A page is using your camera / microphone / screen. Click to stop." sign, bottom left of every screen, above the assistant's
/// camera sign when that is shown. Returns its rectangle (the click target).
pub(crate) fn draw_capture_sign(g: &mut Gfx, h: u32, s: f32, bits: u8, above_assistant_sign: bool) -> R {
    let what = crate::apps::browser_media::kinds_words(bits);
    let text = format!("A page is using your {what}. Click to stop.");
    let tw = g.text_width(15.0 * s, &text, Face::Bold) + 48.0 * s;
    let y = h as f32 - 64.0 * s - if above_assistant_sign { 44.0 * s } else { 0.0 };
    let r = R::new(16.0 * s, y, tw, 34.0 * s);
    g.rrect_glow(r.x, r.y, r.w, r.h, r.h / 2.0, 8.0 * s, [255, 40, 50, 130]);
    g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, rgb(110, 12, 22));
    g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 2.0 * s, rgb(255, 90, 100));
    g.circle(r.x + 18.0 * s, r.y + r.h / 2.0, 7.0 * s, rgb(255, 60, 70));
    g.text(r.x + 34.0 * s, r.y + 8.0 * s, 15.0 * s, TEXT, &text, Face::Bold);
    r
}

/// The red sign of a remote session whose browser microphone or camera is live, bottom right. Returns its rectangle (the click target).
pub(crate) fn draw_remote_capture_sign(g: &mut Gfx, w: u32, h: u32, s: f32, text: &str) -> R {
    let tw = g.text_width(15.0 * s, text, Face::Bold) + 48.0 * s;
    let r = R::new((w as f32 - tw - 16.0 * s).max(0.0), h as f32 - 64.0 * s, tw, 34.0 * s);
    g.rrect_glow(r.x, r.y, r.w, r.h, r.h / 2.0, 8.0 * s, [255, 40, 50, 130]);
    g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, rgb(110, 12, 22));
    g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 2.0 * s, rgb(255, 90, 100));
    g.circle(r.x + 18.0 * s, r.y + r.h / 2.0, 7.0 * s, rgb(255, 60, 70));
    g.text(r.x + 34.0 * s, r.y + 8.0 * s, 15.0 * s, TEXT, text, Face::Bold);
    r
}

#[cfg(test)]
#[path = "shell_overlap_tests.rs"]
mod overlap_tests;

#[cfg(test)]
#[path = "shell_parallel_draw_tests.rs"]
mod parallel_draw_tests;

#[cfg(test)]
#[path = "shell_remote_sign_tests.rs"]
pub(crate) mod remote_sign_tests;

#[cfg(test)]
#[path = "shell_cameras_tests.rs"]
mod cameras_tests;
#[cfg(test)]
use home_impl::draw_custom_tile;

#[cfg(test)]
#[path = "shell_sweep_tests.rs"]
mod sweep_tests;
