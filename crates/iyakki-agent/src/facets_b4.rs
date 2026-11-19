//! The real implementation of `iyakki_agent_api::facets_b4` (F-SPLIT-03, lane alpha.6/split-a): the wrapper runs of the account tool chain.
//! Each method is the body the UI's `tool_chain.rs` had, in the same order, so the chain offers the same tools with the same approvals.

use std::sync::Arc;

use iyakki_agent_api::{ApSaved, ApStart, ChainEnv, PdfParts, RouterParts, ServicesFacetB4, SftpAccounts, WifiParts};
use iyakki_sec::policy::Role;
use iyakki_shared::agent::Tools;

use crate::services::Services;

/// The facet over the services of this install.
pub struct ServicesB4 {
    svc: Arc<Services>,
}

impl Services {
    /// The account tool chain's wrapper runs (what the UI holds instead of `iyakki_agent::updatestools`, `routertools`, ...).
    pub fn b4(self: &Arc<Self>) -> Arc<dyn ServicesFacetB4> {
        Arc::new(ServicesB4 { svc: self.clone() })
    }
}

fn audit_fn(env: &ChainEnv) -> iyakki_shared::gitkeys::AuditFn {
    let (audit, user) = (env.audit.clone(), env.user.clone());
    Arc::new(move |a, d| audit(&user, a, d))
}

impl ServicesFacetB4 for ServicesB4 {
    fn wrap_net_browser_tools(&self, inner: Arc<dyn Tools>, env: &ChainEnv) -> Arc<dyn Tools> {
        let svc = &self.svc;
        let admin = env.policy.manage_settings;
        let tools = if admin { Arc::new(crate::sshtunneltools::SshTunnelTools::new(inner, crate::sshtunnel_mgr::Manager::shared(svc, &env.user, audit_fn(env)))) as Arc<dyn Tools> } else { inner };
        let tools = if admin { Arc::new(crate::domainstools::DomainTools::new(tools, crate::domains::Manager::shared(svc, &env.user, audit_fn(env)))) as Arc<dyn Tools> } else { tools };
        let backend = crate::browsershort::FileBackend { data: svc.dir().to_path_buf(), user: env.user.clone(), audit: audit_fn(env) };
        let tools: Arc<dyn Tools> = Arc::new(crate::browsershort::ShortTools::new(tools, Arc::new(backend)));
        if admin {
            Arc::new(crate::webgraphicstools::GraphicsTools::new(tools, Arc::new(crate::webgraphicstools::RealBackend { data: svc.dir().to_path_buf(), audit: audit_fn(env) })))
        } else {
            tools
        }
    }

    fn wrap_sftp_tools(&self, inner: Arc<dyn Tools>, env: &ChainEnv, accounts: SftpAccounts) -> Arc<dyn Tools> {
        if !env.policy.manage_settings {
            return inner;
        }
        let audit = env.audit.clone();
        let auth = crate::sftp::FnAuth::new(accounts.login, accounts.lookup);
        let sftp = crate::sftp::Sftp::shared(self.svc.dir(), Arc::new(auth), Arc::new(move |e, d| audit("sftp", e, d)));
        Arc::new(crate::sftptools::SftpTools::new(inner, sftp, &env.user))
    }

    fn wrap_run_a_tools(&self, inner: Arc<dyn Tools>, env: &ChainEnv) -> Arc<dyn Tools> {
        let svc = &self.svc;
        let tools: Arc<dyn Tools> = Arc::new(crate::updatestools::UpdatesTools::new(inner, Arc::new(crate::updatestools::RealBackend { svc: svc.clone(), policy: env.policy.clone(), user: env.user.clone(), audit: audit_fn(env) })));
        let tools: Arc<dyn Tools> = Arc::new(crate::imagebuildtools::ImageBuildTools::new(tools, Arc::new(crate::imagebuildtools::RealBackend { svc: svc.clone(), policy: env.policy.clone(), user: env.user.clone(), audit: audit_fn(env) })));
        let tools: Arc<dyn Tools> = Arc::new(crate::k8sexptools::K8sExpTools::new(tools, Arc::new(crate::k8sexptools::RealBackend { svc: svc.clone(), role: env.role, user: env.user.clone(), audit: audit_fn(env) })));
        Arc::new(crate::linuxsandboxtools::LinuxSandboxTools::new(tools, Arc::new(crate::linuxsandboxtools::RealBackend { svc: svc.clone(), policy: env.policy.clone(), user: env.user.clone(), audit: audit_fn(env) })))
    }

    fn wrap_run_b_tools(&self, inner: Arc<dyn Tools>, env: &ChainEnv, file_servers: Vec<iyakki_shared::mcp::ServerSpec>) -> Arc<dyn Tools> {
        let svc = &self.svc;
        let tools: Arc<dyn Tools> = Arc::new(crate::skillstools::SkillsTools::new(inner, Arc::new(crate::skillstools::RealBackend { svc: svc.clone(), policy: env.policy.clone(), user: env.user.clone(), audit: audit_fn(env) })));
        let tools: Arc<dyn Tools> = Arc::new(crate::mcpadmintools::McpAdminTools::new(tools, Arc::new(crate::mcpadmintools::RealBackend { svc: svc.clone(), policy: env.policy.clone(), user: env.user.clone(), file_servers, audit: audit_fn(env) })));
        Arc::new(crate::linuxpodtools::LinuxPodTools::new(tools, Arc::new(crate::linuxpodtools::RealBackend { svc: svc.clone(), policy: env.policy.clone(), user: env.user.clone(), audit: audit_fn(env) })))
    }

    fn wrap_router_tools(&self, inner: Arc<dyn Tools>, env: &ChainEnv, parts: RouterParts) -> Arc<dyn Tools> {
        let svc = &self.svc;
        let (Ok(hub), Ok(who)) = (parts.camera_hub.downcast::<crate::cameras::Hub>(), parts.camera_who.downcast::<crate::cameras::Who>()) else {
            return inner;
        };
        let be = crate::routertools::FileBackend::for_services(svc.clone(), &env.user, audit_fn(env))
            .with_gate(parts.os.router())
            .with_pool(Arc::new(crate::router_pool::ProviderPool::for_services(svc.clone())))
            .with_step_tools(svc.router_step_tools(inner.clone(), Some(parts.os.router())))
            .with_sink(Arc::new(crate::router::ServicesSink { svc: svc.clone(), user: env.user.clone() }))
            .with_senses(svc.router_camera_senses(hub, *who, Some(Arc::new(crate::router::GatedListener { gate: parts.os.router(), record: crate::router::services_recorder(svc.clone(), crate::browsersource_svc::recorders_for(&env.user)) }))))
            .with_verifier(match &parts.project_root {
                Some(root) => svc.router_project_verifier(root),
                None => svc.router_project_verifier(std::path::Path::new("")),
            })
            .with_fleet(svc.router_fleet_runner(parts.os.clone(), parts.os.fleet(), audit_fn(env)));
        Arc::new(crate::routertools::RouterTools::new(inner, Arc::new(be)).with_scope(&parts.scope))
    }

    fn wrap_status_tools(&self, inner: Arc<dyn Tools>, env: &ChainEnv) -> Arc<dyn Tools> {
        let svc = &self.svc;
        let tools: Arc<dyn Tools> = Arc::new(crate::doctortools::DoctorTools::new(inner, Arc::new(crate::doctortools::RealBackend { svc: svc.clone(), policy: env.policy.clone(), user: env.user.clone(), audit_recent: env.audit_recent.clone() })));
        let tools: Arc<dyn Tools> = Arc::new(crate::sessionlogtools::SessionLogTools::new(tools, Arc::new(crate::sessionlogtools::RealBackend { svc: svc.clone(), policy: env.policy.clone(), user: env.user.clone(), audit: audit_fn(env) })));
        let guest = env.role == Role::Guest;
        Arc::new(crate::lockscreentools::LockTools::new(tools, Arc::new(crate::lockscreentools::RealBackend { dir: svc.dir().to_path_buf(), user: env.user.clone(), guest, audit: audit_fn(env) })))
    }

    fn wrap_home_access_tools(&self, inner: Arc<dyn Tools>, env: &ChainEnv) -> Arc<dyn Tools> {
        let svc_v = self.svc.clone();
        let voice_ready: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || ServicesFacetB4::voice_ready(&ServicesB4 { svc: svc_v.clone() }));
        let svc = &self.svc;
        let guest = env.role == Role::Guest;
        let tools: Arc<dyn Tools> = Arc::new(crate::enchometools::EncHomeTools::new(inner, Arc::new(crate::enchometools::RealBackend { dir: svc.dir().to_path_buf(), user: env.user.clone(), guest, audit: audit_fn(env) })));
        let tools: Arc<dyn Tools> = Arc::new(crate::accessibilitytools::AccessTools::new(tools, Arc::new(crate::accessibilitytools::RealBackend { dir: svc.dir().to_path_buf(), user: env.user.clone(), voice_ready, audit: audit_fn(env) })));
        Arc::new(crate::browsera11y::BrowserA11yTools::new(tools, Arc::new(crate::browsera11y::RealBackend { dir: svc.dir().to_path_buf(), user: env.user.clone(), audit: audit_fn(env) })))
    }

    fn wrap_disks_tools(&self, inner: Arc<dyn Tools>, env: &ChainEnv, who: iyakki_shared::disks::Actor) -> Arc<dyn Tools> {
        let svc = &self.svc;
        let tools = if env.role != Role::Guest {
            let engine = Arc::new(iyakki_disksx::disks::Disks::real());
            let runner = Arc::new(iyakki_disksx::disks_helper::ProcRunner::real(&svc.clis.bin_dir()));
            Arc::new(crate::disksunlocktools::DisksCryptTools::new(inner, Arc::new(crate::disksunlocktools::RealBackend { engine, runner, who: who.clone(), dir: svc.dir().to_path_buf(), audit: audit_fn(env) }))) as Arc<dyn Tools>
        } else {
            inner
        };
        if env.role == Role::Admin {
            svc.b0().disks_format_tools(tools, who, audit_fn(env))
        } else {
            tools
        }
    }

    fn wrap_system_tools(&self, inner: Arc<dyn Tools>, env: &ChainEnv, pdf: PdfParts, chat: Arc<dyn iyakki_shared::chat::ChatBackend>) -> Arc<dyn Tools> {
        let svc = &self.svc;
        let admin = env.policy.manage_settings;
        let mut tools = inner;
        if admin {
            tools = Arc::new(crate::componentstools::ComponentsTools::new(tools, Arc::new(crate::componentstools::RealBackend { svc: svc.clone(), user: Some(env.user.clone()) })));
            tools = Arc::new(crate::buildtools::BuildTools::new(tools, Arc::new(crate::buildtools::RealBackend { svc: svc.clone() })));
        }
        // pdf_create is offered unless the model or agent answering writes PDF files itself (`native_pdf_output` in its settings)
        let native = {
            // the agents' settings are read first: reading them takes the settings lock too
            let agents = svc.agents_cfg();
            let st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
            crate::pdfnativetools::is_native(st.get(), &agents, pdf.ext.as_deref())
        };
        tools = Arc::new(crate::pdftools::PdfTools::new(tools, Arc::new(crate::pdftools::RealBackend { policy: env.policy.clone(), user: env.user.clone(), web: pdf.web, downloads: pdf.downloads, can_open_viewer: env.policy.allows_app(iyakki_sec::policy::App::Pdf) })).native_pdf_output(native));
        if admin {
            tools = Arc::new(crate::pdfnativetools::PdfOutputTools::new(tools, Arc::new(crate::pdfnativetools::RealBackend { svc: svc.clone() })));
        }
        tools = Arc::new(crate::tilestools::TilesTools::new(tools, crate::tilestools::TilesTools::builtin_catalog()));
        if admin {
            tools = Arc::new(crate::objectstoragetools::ObjectStorageTools::new(tools, Arc::new(crate::objectstoragetools::RealBackend { svc: svc.clone(), user: env.user.clone() })));
            tools = Arc::new(crate::modelopttools::ModelTools::new(tools, Arc::new(crate::modelopttools::RealBackend { svc: svc.clone() })));
            tools = Arc::new(crate::ollamatools::OllamaTools::new(tools, Arc::new(crate::ollamatools::RealBackend { svc: svc.clone(), user: env.user.clone() })));
        }
        if env.policy.allows_app(iyakki_sec::policy::App::Rag) {
            let rag = crate::ragtools::RagToolsEnv::for_services(svc, &env.user, &env.policy, chat, audit_fn(env));
            tools = Arc::new(crate::ragtools::RagTools::new(tools, rag));
        }
        if admin {
            tools = Arc::new(crate::quickuninstalltools::QuickUninstallTools::new(tools, Arc::new(crate::quickuninstalltools::RealBackend { svc: svc.clone(), user: env.user.clone() })));
            tools = Arc::new(crate::lemonadecomputetools::LemonadeComputeTools::new(tools, Arc::new(crate::lemonadecomputetools::RealBackend { svc: svc.clone(), user: env.user.clone() })));
            tools = Arc::new(crate::thiscomputertools::ThisComputerTools::new(tools, Arc::new(crate::thiscomputertools::RealBackend { svc: svc.clone() })));
        }
        tools = Arc::new(crate::dictfixtools::DictationWordingTools::new(tools, Arc::new(crate::dictfixtools::RealBackend { svc: svc.clone(), audit: audit_fn(env), can_change: admin })));
        Arc::new(crate::voiceassisttools::VoiceAssistantTools::new(tools, Arc::new(crate::voiceassisttools::RealBackend { svc: svc.clone(), audit: audit_fn(env), can_change: admin })))
    }

    fn wrap_vault_routing_tools(&self, inner: Arc<dyn Tools>, env: &ChainEnv) -> Arc<dyn Tools> {
        let svc = &self.svc;
        let tools = if env.policy.manage_settings { Arc::new(crate::sharedvaulttools::SharedVaultTools::new(inner, svc.dir().to_path_buf(), &env.user)) as Arc<dyn Tools> } else { inner };
        let tools: Arc<dyn Tools> = Arc::new(crate::cloudstoragetools::CloudStorageTools::new(tools, svc.dir().to_path_buf(), &env.user));
        let s2 = svc.clone();
        Arc::new(crate::toolrouting::ToolRoutingTools::new(tools, svc.dir().to_path_buf(), &env.user, Arc::new(move || s2.settings.lock().unwrap_or_else(|e| e.into_inner()).get().no_tool_routing)))
    }

    fn settings_entries_a(&self, base: &iyakki_shared::settingstools::Env) -> Vec<iyakki_shared::settingstools::Entry> {
        let Some(env) = crate::settingstools::Env::from_base(base) else { return Vec::new() };
        let key_present = |svc: Arc<Services>| -> Arc<dyn Fn() -> bool + Send + Sync> {
            Arc::new(move || svc.secrets.resolve(iyakki_shared::gemini::KEY_REF, None::<&iyakki_shared::vault::LocalVault>).is_ok())
        };
        let mut out = Vec::new();
        out.extend(crate::settingstools_views::entries(&env));
        out.extend(crate::settingstools_backup::entries(&env));
        out.extend(crate::settingstools_backup_more::entries(&env));
        out.extend(crate::settingstools_notifs::entries(&env));
        out.extend(crate::settingstools_saver::entries(&env));
        out.extend(crate::settingstools_wasmtiles::entries(&env));
        out.extend(crate::settingstools_startup::entries(&env));
        out.extend(crate::settingstools_gemini::entries(&env, key_present(self.svc.clone())));
        out.extend(crate::settingstools_geminivoice::entries(&env, key_present(self.svc.clone())));
        out.extend(crate::settingstools_sound::entries(&env));
        out
    }

    fn settings_entries_b(&self, base: &iyakki_shared::settingstools::Env, linux: iyakki_shared::settingstools_linux::Access, wifi: WifiParts) -> Vec<iyakki_shared::settingstools::Entry> {
        let Some(env) = crate::settingstools::Env::from_base(base) else { return Vec::new() };
        let mut out = Vec::new();
        out.extend(crate::settingstools_memory::entries(&env));
        out.extend(crate::settingstools_ytdlp::entries(&env));
        out.extend(crate::settingstools_cast::entries(&env));
        out.extend(crate::settingstools_camtrust::entries(&env));
        out.extend(crate::settingstools_linux::entries(&env, linux));
        out.extend(crate::settingstools_access::entries(&env));
        out.extend(crate::settingstools_machine::entries(&env));
        out.extend(crate::settingstools_podtls::entries(&env));
        out.extend(crate::settingstools_net::entries(&env, iyakki_fw::service::Handle::Global));
        out.extend(crate::settingstools_wifi::entries(&env, wifi.ctl, wifi.read));
        out
    }

    fn ssh_tunnels(&self, user: &str, audit: iyakki_agent_api::AuditLineB4) -> Arc<dyn iyakki_agent_api::SshTunnels> {
        Arc::new(TunHandle(crate::sshtunnel_mgr::Manager::shared(&self.svc, user, audit)))
    }
    fn ssh_tunnels_start_for_login(&self, user: &str, audit: iyakki_agent_api::AuditLineB4) {
        crate::sshtunnel_mgr::start_for_login(&self.svc, user, audit)
    }
    fn ssh_tunnels_on_logout(&self, user: &str) {
        crate::sshtunnel_mgr::on_logout(&self.svc, user)
    }

    fn autopilot_start(&self, user: &str, chat: Arc<dyn iyakki_shared::chat::ChatBackend>, tools: Arc<dyn Tools>, base: Vec<iyakki_shared::chat::Message>, limits: iyakki_shared::agent::RunLimits, assist: Box<dyn std::any::Any + Send>, start: ApStart) -> iyakki_shared::agent::RunHandle {
        let Ok(assist) = assist.downcast::<crate::assistrun::Assist>() else {
            let (tx, rx) = std::sync::mpsc::channel();
            let (dtx, _drx) = std::sync::mpsc::channel();
            let _ = tx.send(iyakki_shared::agent::RunEvent::Failed { error: "the chat's assist settings are not this core's".into() });
            return iyakki_shared::agent::RunHandle::from_parts(rx, dtx);
        };
        let mut ctx = crate::autopilot::Context::new(&start.root, &start.features, &start.goal);
        ctx.resume = start.resume;
        ctx.interjection = start.interjection;
        ctx.session = start.session;
        ctx.state = start.state;
        ctx.audit = start.audit;
        ctx.notify = start.notify;
        ctx.executor = start.executor;
        if let Some((name, session)) = start.history {
            ctx.history = Some(iyakki_memx::history::History::new(self.svc.kb.clone(), user, &name, session.as_deref()));
        }
        crate::autopilot::start(chat, tools, base, limits, *assist, ctx)
    }

    fn store(&self) -> Option<Arc<dyn iyakki_agent_api::StoreApi>> {
        Some(crate::store_api::StoreHandle::new(&self.svc))
    }

    fn camera_tools(&self, inner: Arc<dyn Tools>, hub: Arc<dyn std::any::Any + Send + Sync>, who: Box<dyn std::any::Any + Send>) -> Arc<dyn Tools> {
        match (hub.downcast::<crate::cameras::Hub>(), who.downcast::<crate::cameras::Who>()) {
            (Ok(hub), Ok(who)) => crate::camsource::camera_tools(inner, hub, *who, &self.svc),
            _ => inner,
        }
    }
    fn camera_resume(&self, hub: Arc<dyn std::any::Any + Send + Sync>) {
        if let Ok(hub) = hub.downcast::<crate::cameras::Hub>() {
            crate::camsource::resume(&self.svc, &hub);
        }
    }
    fn camera_source(&self, hub: Arc<dyn std::any::Any + Send + Sync>, who: Box<dyn std::any::Any + Send>) -> Option<Arc<dyn iyakki_tile_api::cameras::CameraSource>> {
        match (hub.downcast::<crate::cameras::Hub>(), who.downcast::<crate::cameras::Who>()) {
            (Ok(hub), Ok(who)) => Some(Arc::new(crate::camsource::HubSource { hub, who: *who })),
            _ => None,
        }
    }

    fn usage_source(&self) -> Option<Arc<dyn iyakki_tile_api::usage::UsageSource>> {
        Some(Arc::new(UsageHandle(self.svc.clone())))
    }
    fn metered_chat(&self, chat: Arc<dyn iyakki_shared::chat::ChatBackend>, user: &str) -> Arc<dyn iyakki_shared::chat::ChatBackend> {
        Arc::new(crate::usage::Metered::new(chat, self.svc.clone(), user))
    }

    fn voice_assistant_spawn(&self, user: &str, policy: iyakki_sec::policy::Policy, host: Arc<dyn iyakki_core_api::HostApi>, mcp: Vec<iyakki_shared::mcp::ServerSpec>, chat: Arc<dyn iyakki_shared::chat::ChatBackend>, audit: Arc<dyn Fn(&str, &str, &str) + Send + Sync>, recorders: Arc<dyn iyakki_shared::voicedesk::Recorders>) -> Option<Box<dyn iyakki_agent_api::MineWorker>> {
        use iyakki_agent_api::ServicesB1;
        const BASE_SYSTEM: &str = "You are IYAKKI's built-in assistant on this computer. Help the person with what they ask, using your tools when that is the right way, and answer briefly.";
        let svc = self.svc.clone();
        let (user, can_change) = (user.to_string(), policy.manage_settings);
        let (svc2, user2) = (svc.clone(), user.clone());
        let start: crate::voiceassist_ports::StartRun = Box::new(move |history| {
            let tools = ServicesB1::user_tools(&svc2, &user2, policy.clone(), host.clone(), mcp.clone(), None, None);
            let (a2, u2) = (audit.clone(), user2.clone());
            let backend = crate::voiceassisttools::RealBackend { svc: svc2.clone(), audit: Arc::new(move |a, d| a2(&u2, a, d)), can_change };
            let tools: Arc<dyn Tools> = Arc::new(crate::voiceassisttools::VoiceAssistantTools::new(tools, Arc::new(backend)));
            let limits = iyakki_shared::agent::RunLimits { approval_timeout: std::time::Duration::from_secs(1800), ..iyakki_shared::agent::RunLimits::default() };
            crate::agent::start_with(chat.clone(), tools, history, limits, crate::assistrun::Assist::off())
        });
        let ports = iyakki_shared::voiceassist::Ports {
            audio: Box::new(crate::voiceassist_ports::RecorderAudio::new(recorders, svc.clone())),
            stt: ServicesB1::voice_stt(&svc),
            speaker: ServicesB1::system_speaker(&svc),
            agent: Box::new(crate::voiceassist_ports::RunAgent::new(&format!("{BASE_SYSTEM}{}", crate::voiceassisttools::PROMPT_RULE), start)),
            log: ServicesB1::voice_turn_log(&svc, &user),
        };
        let s2 = svc.clone();
        let live = Box::new(move || {
            let v = s2.voice_settings();
            iyakki_shared::voiceassist_ports::Live { muted: v.my_muted, follow_ms: v.my_follow_ms as u64, pause_ms: v.pause_ms as u64, spoken_approvals: v.spoken_approvals }
        });
        Some(Box::new(MineHandle(crate::voiceassist_ports::spawn(ports, iyakki_shared::voiceassist::Config::default(), live, 50))))
    }

    fn voice_ready(&self) -> bool {
        let svc = &self.svc;
        svc.voice_settings().tts.is_some_and(|e| !matches!(e, iyakki_shared::voice::SpeechEngine::Browser)) || svc.host_tts.report().is_some_and(|r| r.engines.iter().any(|e| e.usable()))
    }

    fn camera_hub(&self, usb: Box<dyn std::any::Any + Send>, people: Arc<dyn Fn() -> Vec<String> + Send + Sync>, encode: Arc<dyn Fn(u32, u32, &[u8]) -> Vec<u8> + Send + Sync>, after_save: Arc<dyn Fn(&std::path::Path) + Send + Sync>, audit: Arc<dyn Fn(&str, &str, &str) + Send + Sync>, host_name: &str) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        let usb = *usb.downcast::<Arc<dyn crate::cameras::UsbProvider>>().ok()?;
        let data = self.svc.dir().to_path_buf();
        let hub = crate::cameras::shared(crate::cameras::Env {
            data: data.clone(),
            secrets: Arc::new(crate::cameras::VaultCamSecrets(self.svc.clone())),
            people,
            usb,
            h264: Arc::new(crate::cameras::NoH264),
            encode,
            after_save,
            audit,
            backoff_ms: 1000,
            idle: crate::cameras::IDLE_STOP,
        });
        // every use of this computer's own cameras goes through the broker, which decides and writes the access log (CORE-ACCESS-MODEL.md, slice 2)
        if !hub.has_broker() {
            let (broker, _) = crate::accessbroker::local(&data, None);
            hub.set_broker(broker, host_name);
        }
        Some(hub)
    }

    fn project_kit(&self, spec: iyakki_agent_api::ProjectKitSpec) -> Option<Arc<dyn iyakki_agent_api::ProjectKit>> {
        let k8s = spec.cluster.map(|c| {
            let audit = spec.audit.clone();
            let mut k = crate::k8stools::K8sTools::new(c.runner, c.profile, &c.namespace, &spec.name, &spec.root, Arc::new(move |a, d| audit(&format!("k8s.{a}"), d)));
            k.cluster_phrase = c.phrase;
            Arc::new(k)
        });
        Some(Arc::new(ProjectKitHandle { svc: self.svc.clone(), user: spec.user, name: spec.name, root: spec.root, features: spec.features, audit: spec.audit, session: spec.session, k8s }))
    }
    fn wrap_job_tools(&self, inner: Arc<dyn Tools>, user: &str, policy: iyakki_sec::policy::Policy, cwd: &std::path::Path, workspace: Option<(&str, &std::path::Path)>) -> Arc<dyn Tools> {
        Arc::new(crate::jobtools::JobTools::new(inner, self.svc.jobs.clone(), user, policy, cwd, workspace))
    }

    fn chat_run_start(&self, user: &str, prompt: &str, chat: Arc<dyn iyakki_shared::chat::ChatBackend>, tools: Arc<dyn Tools>, history: Vec<iyakki_shared::chat::Message>, limits: iyakki_shared::agent::RunLimits, assist: Box<dyn std::any::Any + Send>) -> Option<Box<dyn iyakki_agent_api::ChatRun>> {
        let assist = *assist.downcast::<crate::assistrun::Assist>().ok()?;
        Some(Box::new(crate::runs::RunSession::start_with(Some(self.svc.runs.clone()), user, prompt, chat, tools, history, limits, assist)))
    }
    fn chat_run_from_handle(&self, user: &str, prompt: &str, handle: iyakki_shared::agent::RunHandle) -> Option<Box<dyn iyakki_agent_api::ChatRun>> {
        Some(Box::new(crate::runs::RunSession::from_handle(Some(self.svc.runs.clone()), user, prompt, handle)))
    }

    fn voicepack_remove(&self, delete: &dyn Fn(&str) -> Result<(), String>) -> Result<String, String> {
        crate::voicepack::remove(&self.svc, delete)
    }

    fn wrap_bluetooth_tools(&self, inner: Arc<dyn Tools>, env: &ChainEnv) -> Arc<dyn Tools> {
        if env.policy.manage_settings {
            Arc::new(crate::bluetoothpair::BluetoothPairTools::real(Arc::new(crate::btaudiotools::BtAudioTools::real(Arc::new(crate::bluetoothtools::BluetoothTools::real(inner))))))
        } else {
            inner
        }
    }
}

/// The stateless helpers.
pub struct RealHelpersB4;

impl iyakki_agent_api::HelpersB4 for RealHelpersB4 {
    fn provider_plain_error(&self, provider: &str, raw: &str) -> String {
        crate::providererr::plain_error(provider, raw)
    }
    fn locality_label(&self, url: &str) -> &'static str {
        crate::speechtime::locality(url).label()
    }
    fn speech_server_of(&self, url: &str) -> String {
        crate::speechtime::server_of(url)
    }
    fn touch_panel_names(&self, sys: &std::path::Path) -> Vec<String> {
        crate::touchprobe::panels(sys).into_iter().map(|p| p.name).collect()
    }
    fn touch_probe(&self, sys: &std::path::Path, level: u8) -> String {
        crate::touchprobe::probe(sys, level)
    }
    fn touch_reset_now(&self, uptime_secs: u64) -> String {
        crate::touchrecover::reset_now(uptime_secs)
    }
    fn vault_pending_list(&self, data: &std::path::Path) -> Vec<(String, String)> {
        crate::vaultpending::list(data).into_iter().map(|p| (p.user, p.what)).collect()
    }
    fn voicepack_delete_local_model(&self, model: &str) -> Result<(), String> {
        crate::voicepack::delete_local_model(model)
    }
    fn autopilot_saved(&self, state_path: &std::path::Path) -> Option<ApSaved> {
        crate::autopilot::State::load(state_path).map(|s| ApSaved { status: s.status, goal: s.goal, step: s.step, reason: s.reason })
    }
    fn pf_preflight(&self, dir: &std::path::Path) -> iyakki_shared::store_publishflow::Preflight {
        crate::store::publishflow::preflight(dir)
    }
    fn pf_header_hint(&self, dir: &std::path::Path) -> Option<String> {
        crate::store::publishflow::header_hint(dir)
    }
    fn tile_run_tests(&self, dir: &std::path::Path) -> Result<iyakki_shared::store_tile::TestReport, String> {
        crate::store::tile::run_tests(dir)
    }
    fn camera_sign_names(&self, data: &std::path::Path) -> Vec<String> {
        crate::camsource::sign_names(data)
    }
    fn camera_stop_all_signs(&self, data: &std::path::Path, user: &str) -> usize {
        crate::camsource::stop_all_signs(data, user)
    }
    fn workspace_prompt(&self, name: &str, root: &std::path::Path, features_file: &str, cluster: Option<(&iyakki_shared::devcluster::ClusterProfile, &str)>) -> String {
        crate::boardtools::workspace_prompt_with_cluster(name, root, features_file, cluster)
    }
    fn project_pinned_skills(&self, root: &std::path::Path, stack_override: &str, request: &str) -> Vec<String> {
        crate::projectstate::detect_stack(root, stack_override).pinned_skills(request).iter().map(|s| s.to_string()).collect()
    }
    fn calendar_ring(&self, play_cmd: &str) -> Result<Box<dyn iyakki_shared::voicedesk::Playback>, String> {
        crate::voice::Player::start_as(crate::mixer::SourceKind::Alarm, play_cmd, &iyakki_shared::calendar::alarm_wav()).map(|p| Box::new(p) as Box<dyn iyakki_shared::voicedesk::Playback>)
    }
    fn sound_outputs(&self) -> Vec<iyakki_shared::audioout::Output> {
        crate::doctor_sys::sound_outputs()
    }
    fn uptime_secs(&self) -> u64 {
        crate::doctor_sys::uptime_secs()
    }
    fn doctor_slow_probe(&self, input: iyakki_shared::doctor_gather::SlowIn) -> iyakki_shared::doctor_gather::SlowOut {
        crate::doctor_gather::slow_probe(input)
    }
    fn radio_languages(&self) -> &'static [iyakki_tile_api::radio::Language] {
        crate::radio_stations::LANGUAGES
    }
    fn data_tab_context(&self, user: &str) -> Option<String> {
        crate::datatools::tab_context(user)
    }
    fn upnp_start_for(&self, data_dir: &std::path::Path, user: &str, audit: iyakki_agent_api::AuditLineB4) -> Result<String, String> {
        crate::upnp_serve::start_for(data_dir, user, audit)
    }
    fn upnp_subscribers(&self, user: &str) -> (String, Vec<(String, String, i64)>) {
        let list = crate::upnp_serve::subscriptions_for(user);
        (crate::upnp_serve::subscribers_text(&list), list.iter().map(|s| (s.peer.to_string(), s.service.to_string(), s.left_secs)).collect())
    }
}

impl iyakki_agent_api::AgentFacetsB4 for crate::AgentApiImpl {
    fn helpers_b4(&self) -> &dyn iyakki_agent_api::HelpersB4 {
        &RealHelpersB4
    }
}

/// The shared manager of a person behind the facet.
pub struct TunHandle(pub Arc<crate::sshtunnel_mgr::Manager>);

impl iyakki_agent_api::SshTunnels for TunHandle {
    fn tunnels(&self) -> Vec<iyakki_shared::sshtunnel::Tunnel> {
        self.0.tunnels()
    }
    fn get(&self, id: &str) -> Option<iyakki_shared::sshtunnel::Tunnel> {
        self.0.get(id)
    }
    fn rows(&self) -> Vec<iyakki_shared::sshtunnel_mgr::Row> {
        self.0.rows()
    }
    fn save(&self, t: iyakki_shared::sshtunnel::Tunnel, new_key: bool) -> Result<iyakki_shared::sshtunnel::Tunnel, String> {
        self.0.save(t, new_key)
    }
    fn set_enabled(&self, id: &str, on: bool) -> Result<iyakki_shared::sshtunnel::Tunnel, String> {
        self.0.set_enabled(id, on)
    }
    fn retry(&self, id: &str) {
        self.0.retry(id)
    }
    fn remove(&self, id: &str) -> Result<Vec<String>, String> {
        self.0.remove(id)
    }
    fn trust(&self, host: &str, port: u16, fingerprint: &str) -> Result<String, String> {
        self.0.trust(host, port, fingerprint)
    }
    fn tick(&self, now: u64) {
        self.0.tick(now)
    }
    fn ensure_supervised(&self) {
        // the real one needs the shared handle; a facet handle is always the shared one
        self.0.ensure_supervised()
    }
    fn test(&self, id: &str, now: u64) -> iyakki_shared::sshtunnel_mgr::TestReport {
        self.0.test(id, now)
    }
    fn key_info(&self, id: &str) -> Option<iyakki_shared::gitkeys::KeyInfo> {
        self.0.keys().key(id)
    }
    fn key_ids(&self) -> Vec<String> {
        self.0.keys().keys().into_iter().map(|k| k.id).collect()
    }
}

/// The real usage log, in the machine's time zone (the Usage tile's source).
pub struct UsageHandle(pub Arc<Services>);

fn range_to_agent(r: iyakki_tile_api::usage::Range) -> crate::usage::Range {
    use iyakki_tile_api::usage::Range as R;
    match r {
        R::Day => crate::usage::Range::Day,
        R::Week => crate::usage::Range::Week,
        R::Month => crate::usage::Range::Month,
        R::Months => crate::usage::Range::Months,
        R::Year => crate::usage::Range::Year,
    }
}

impl iyakki_tile_api::usage::UsageSource for UsageHandle {
    fn now_unix(&self) -> i64 {
        iyakki_clock::now_unix()
    }
    fn series(&self, range: iyakki_tile_api::usage::Range, now_unix: i64, user: Option<&str>) -> iyakki_tile_api::usage::Series {
        let s = self.0.usage.series(range_to_agent(range), now_unix, &iyakki_clock::Zone::system(), user);
        use iyakki_tile_api::usage as api;
        api::Series {
            models: s.models,
            buckets: s.buckets.into_iter().map(|b| api::Bucket { label: b.label, full: b.full, start: b.start, end: b.end, by_model: b.by_model, input: b.input, output: b.output, calls: b.calls }).collect(),
            totals: s.totals,
            input: s.input,
            output: s.output,
            calls: s.calls,
        }
    }
}

/// The voice assistant worker behind the facet.
pub struct MineHandle(pub crate::voiceassist_ports::Worker);

impl iyakki_agent_api::MineWorker for MineHandle {
    fn send(&self, cmd: iyakki_shared::voiceassist_ports::Cmd) {
        self.0.send(cmd)
    }
    fn status(&self) -> iyakki_shared::voiceassist::Status {
        self.0.status()
    }
    fn wake(&self, text: &str) {
        self.0.wake(text)
    }
}

/// The project tools of one run behind the facet.
pub struct ProjectKitHandle {
    svc: Arc<Services>,
    user: String,
    name: String,
    root: std::path::PathBuf,
    features: std::path::PathBuf,
    audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    session: Option<String>,
    k8s: Option<Arc<crate::k8stools::K8sTools>>,
}

impl ProjectKitHandle {
    fn history(&self) -> Option<iyakki_memx::history::History> {
        let off = self.svc.settings.lock().unwrap_or_else(|e| e.into_inner()).get().no_workspace_history;
        (!off).then(|| iyakki_memx::history::History::new(self.svc.kb.clone(), &self.user, &self.name, self.session.as_deref()))
    }
}

impl iyakki_agent_api::ProjectKit for ProjectKitHandle {
    fn cluster_line(&self) -> Option<String> {
        self.k8s.as_ref().map(|k| format!("{} · namespace {}", k.profile.describe(), k.ns))
    }
    fn state(&self, stack_override: &str, test: &str, run: &str, build: &str) -> String {
        let history = self.history();
        crate::projectstate::build(&crate::projectstate::Input { name: &self.name, root: &self.root, features: &self.features, stack_override, test, run, build, cluster: self.cluster_line(), history: history.as_ref() })
    }
    fn wrap(&self, base: Arc<dyn Tools>, platform: Option<Arc<dyn std::any::Any + Send + Sync>>) -> Arc<dyn Tools> {
        let audit = self.audit.clone();
        let mut pt = crate::boardtools::ProjectTools::new(base, &self.root, &self.name, &self.features).with_project_test_command(&[&self.root]);
        pt = pt.with_audit(Arc::new(move |k, d| audit(k, d)));
        if let Some(h) = self.history() {
            pt = pt.with_history(h);
        }
        if let Some(k) = &self.k8s {
            pt = pt.with_k8s(k.clone());
        }
        if let Some(pl) = platform.and_then(|p| p.downcast::<crate::platform::tools::PlatformTools>().ok()) {
            pt = pt.with_platform(pl);
        }
        Arc::new(pt)
    }
}

impl iyakki_agent_api::ChatRun for crate::runs::RunSession {
    fn id(&self) -> String {
        crate::runs::RunSession::id(self).to_string()
    }
    fn is_done(&self) -> bool {
        crate::runs::RunSession::is_done(self)
    }
    fn poll(&mut self) -> Vec<iyakki_shared::agent::RunEvent> {
        crate::runs::RunSession::poll(self)
    }
    fn decide(&mut self, call_id: &str, v: iyakki_shared::agent::Verdict) {
        crate::runs::RunSession::decide(self, call_id, v)
    }
    fn cancel(&self) {
        crate::runs::RunSession::cancel(self)
    }
    fn pause(&self) {
        crate::runs::RunSession::pause(self)
    }
    fn in_session(self: Box<Self>, session: &str) -> Box<dyn iyakki_agent_api::ChatRun> {
        Box::new((*self).in_session(session))
    }
}
