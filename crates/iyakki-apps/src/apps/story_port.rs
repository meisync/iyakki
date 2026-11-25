//! What the TALES tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! The tile (`story/`) names no `Services`, no session, no audit log and no facet: it asks a [`TalesPort`] for the signed-in person and what
//! their account allows, the story library and the PDF writer, the chat model and the picture models, the speech engines and the external
//! coding agents, and for one [`TalesWorker`] to hand to the threads it starts (a story being written, a picture being drawn, a link being
//! fetched). The real port is `impl TalesPort for AppCx` below, the only place that reads the session and calls the services (so the
//! tile's old tests, which hand it an `AppCx`, still run through it); a fake port in `story/port_tests.rs` hands the tile in-memory answers.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_chat_model::ChatBackend;
use iyakki_provider_kind::lemonade::Model as LemonadeModel;
use iyakki_provider_kind::PullStatus;
use iyakki_story_kit::storyillus::Cand;
use iyakki_story_kit::{QuickEngine, Story, StoryOptions, StoryPdf, StoryStore, StoredPic, Synth};
use iyakki_story_model::ModelChoice;
use iyakki_tile_tales::port::{AgentEntry, AgentRunReq, ImageOut, LinkedPic, TalesPort, TalesWorker};
use iyakki_voice_model::voice::{SpeakOpts, SpeechEngine, VoiceSettings};
use iyakki_voice_model::hosttts::HostTtsApi;
use iyakki_chatview::model_picker::{Current, ModelList, TopRow};

use super::AppCx;
use crate::gfx::Gfx;
use crate::ui::R;

/// The agent's own run request for what the tile asks: no tools, one step, fifteen minutes at most, never auto-approved.
fn run_spec(req: AgentRunReq) -> iyakki_agent_api::ExtRunSpec {
    let system = req.system;
    iyakki_agent_api::ExtRunSpec {
        agent: req.agent,
        workspace: req.workspace,
        prompt: req.prompt,
        system: Arc::new(move || system.clone()),
        model: req.model,
        resume: None,
        tools: Arc::new(iyakki_shared::agent::NoTools),
        limits: iyakki_shared::agent::RunLimits { max_seconds: 900, max_steps: 1, ..Default::default() },
        allow_auto: false,
        key: req.key.map(|k| zeroize::Zeroizing::new((**k).clone())),
        audit: req.audit,
        exe: req.exe,
        on_session: Arc::new(|_| {}),
        on_usage: req.on_usage,
        no_tools: true,
    }
}

/// The agent's run handle as a feed of plain messages (the tile follows it with `iyakki_story_kit::follow_run`).
struct HandleFeed(iyakki_shared::agent::RunHandle);

impl iyakki_story_kit::RunFeed for HandleFeed {
    fn recv(&self, wait: std::time::Duration) -> Result<iyakki_story_kit::RunMsg, std::sync::mpsc::RecvTimeoutError> {
        use iyakki_shared::agent::RunEvent;
        use iyakki_story_kit::RunMsg;
        Ok(match self.0.events.recv_timeout(wait)? {
            RunEvent::Delta { text } => RunMsg::Delta(text),
            RunEvent::Thinking { text, .. } => RunMsg::Thinking(text),
            RunEvent::Model { text, calls } => RunMsg::Model { text, calls: calls.into_iter().map(|c| c.id).collect() },
            RunEvent::Approval { id, .. } => RunMsg::Approval(id),
            RunEvent::Final { text } => RunMsg::Final(text),
            RunEvent::Failed { error } => RunMsg::Failed(error),
            RunEvent::Stopped { .. } => RunMsg::Stopped,
            _ => RunMsg::Other,
        })
    }
    fn cancel(&self) {
        self.0.cancel();
    }
    fn deny(&self, id: &str) {
        self.0.decide(id, iyakki_shared::agent::Verdict::Deny);
    }
}

/// The worker over the real services.
struct CxWorker {
    svc: Arc<dyn iyakki_agent_api::ServicesHandle>,
    web: Arc<dyn iyakki_web::Fetch>,
    policy: iyakki_sec::policy::Policy,
    agent: Arc<dyn iyakki_agent_api::AgentApi>,
}

impl TalesWorker for CxWorker {
    fn provider_is_lemonade(&self, provider: &str) -> bool {
        self.svc.provider_is_lemonade(provider)
    }
    fn lemonade_live_model(&self, provider: &str, model: &str) -> Result<Option<LemonadeModel>, String> {
        self.svc.lemonade_live_model(provider, model)
    }
    fn lemonade_health(&self, provider: &str) -> Result<(String, Vec<String>), String> {
        self.svc.lemonade_health(provider)
    }
    fn generate_image(&self, model: Option<(&str, &str)>, prompt: &str, size: &str, params: &iyakki_provider_kind::ImageParams) -> ImageOut {
        self.svc.generate_image_traced(model, prompt, size, params)
    }
    fn start_agent(&self, req: AgentRunReq) -> Box<dyn iyakki_story_kit::RunFeed> {
        Box::new(HandleFeed(self.svc.extagent_facet().start(run_spec(req))))
    }
    fn fetch_link(&self, url: &str, allow_video: bool) -> Result<LinkedPic, String> {
        self.agent.scene().fetch_link(url, self.web.as_ref(), &|h: &str| self.policy.allows_host(h), allow_video)
    }
}

impl TalesPort for AppCx {
    fn is_admin(&self) -> bool {
        self.session.policy.manage_settings
    }
    fn may_run_commands(&self) -> bool {
        self.session.policy.cli == iyakki_sec::policy::CliLevel::Full
    }
    fn voice_allowed(&self) -> bool {
        self.session.policy.voice
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.record(&self.session.user, action, subject);
    }
    fn audit_fn(&self) -> Arc<dyn Fn(&str, &str) + Send + Sync> {
        let (audit, user) = (self.audit.clone(), self.session.user.clone());
        Arc::new(move |k, d| audit.record(&user, k, d))
    }
    fn usage_fn(&self) -> Arc<dyn Fn(&str, &str, u64, u64) + Send + Sync> {
        let (ai, user) = (self.ai(), self.session.user.clone());
        Arc::new(move |l, m, i, o| ai.usage_record(&user, l, m, i, o))
    }
    fn clock(&self) -> Arc<dyn Fn() -> u64 + Send + Sync> {
        Arc::new(|| iyakki_clock::now_unix().max(0) as u64)
    }
    fn data_dir(&self) -> PathBuf {
        AppCx::data_dir(self).to_path_buf()
    }
    fn rewording(&self) -> Arc<dyn Fn(&str) -> String + Send + Sync> {
        let os = self.os.clone();
        Arc::new(move |m| os.disks().reword_failure(m))
    }
    fn memory(&self) -> Option<iyakki_sysinfo_model::Memory> {
        Some(crate::host().memory())
    }

    fn story_store(&self) -> StoryStore {
        self.b5().story_store(&self.session.policy, &self.session.user, AppCx::data_dir(self)).expect("the core serves the story store")
    }
    fn story_pdf(&self, story: &Story, opts: &StoryOptions, pics: &[StoredPic], now: i64) -> Result<StoryPdf, String> {
        self.b5().story_pdf(story, opts, pics, true, Some(self.agent.pdftools_pdf_date(now)))
    }
    fn asks_for_pdf(&self, text: &str) -> bool {
        self.agent.storypdf_asks_for_pdf(text)
    }
    fn asks_for_link(&self, text: &str, scenes: usize) -> Option<(usize, String)> {
        self.agent.scene().asks_for_link(text, scenes)
    }
    fn new_reader(&self) -> Arc<dyn iyakki_story_model::StoryReader> {
        crate::new_reader()
    }

    fn chat(&self) -> Arc<dyn ChatBackend> {
        self.chat.clone()
    }
    fn chat_backend_for(&self, choice: ModelChoice) -> Arc<dyn ChatBackend> {
        AppCx::chat_backend_for(self, choice)
    }
    fn chat_choice(&self) -> Option<ModelChoice> {
        self.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get().chat.clone()
    }
    fn text_models(&self) -> Vec<(String, String)> {
        let st = self.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        st.get().providers.iter().filter(|p| p.enabled).flat_map(|p| p.models.iter().map(move |m| (p.id.clone(), m.clone()))).collect()
    }
    fn quick_engines(&self, path: &str, voice_dirs: &[PathBuf]) -> Vec<QuickEngine> {
        let providers = self.settings().lock().map(|s| s.get().providers.clone()).unwrap_or_default();
        iyakki_shared::story::quick_engines(&providers, path, voice_dirs)
    }
    fn set_speech_engine(&self, engine: SpeechEngine) -> Result<(), String> {
        self.settings().lock().map_err(|e| e.to_string()).and_then(|mut s| s.update(|x| x.voice.tts = Some(engine.clone())).map_err(|e| e.to_string()))
    }
    fn voice_settings(&self) -> VoiceSettings {
        AppCx::voice_settings(self)
    }
    fn model_list(&self, plain: Vec<(String, String)>, top: Option<TopRow>, agents: bool, current: Current, title: &str, hint: &str) -> Option<ModelList> {
        super::model_picker::build(self, plain, top, agents, current, title, hint)
    }
    fn model_draw(&self, g: &mut Gfx, area: R, scale: f32, list: &mut ModelList) {
        super::model_picker::draw(g, area, scale, self, list)
    }
    fn model_remember_custom(&self, agent: &str, model: &str) {
        super::model_picker::remember_custom(self, agent, model)
    }

    fn agent_entry(&self, id: &str) -> Option<AgentEntry> {
        self.extagent_facet().entries().into_iter().find(|e| e.id == id).map(|e| AgentEntry { id: e.id, label: e.label, chip: e.chip, ready: e.ready })
    }
    fn agent_known(&self, id: &str) -> bool {
        self.agents_cfg().agent(id).is_some()
    }
    fn agent_label(&self, id: &str) -> Option<String> {
        self.agents_cfg().agent(id).map(|a| a.label())
    }
    fn agent_advisor(&self, id: &str) -> bool {
        self.agents_cfg().agent(id).is_some_and(|a| a.mode() == iyakki_extagentx::Mode::Advisor)
    }
    fn agent_check(&self, id: &str) -> Result<(), String> {
        match self.ext_status().get(id) {
            Some(st) if !st.ready() => Err(st.fix.clone()),
            Some(_) => Ok(()),
            None => {
                self.ext_status().refresh(self.agents_cfg());
                Ok(())
            }
        }
    }
    fn agent_model_label(&self, id: &str, model: &str) -> String {
        let cfg = self.agents_cfg();
        cfg.agent(id).and_then(|a| iyakki_shared::extagent::models_with(self.ext_status().cli_models(&a.as_ref().id()), a.as_ref()).into_iter().find(|m| m.id == model)).map_or(model.to_string(), |m| m.label)
    }
    fn agent_key_ref(&self, id: &str) -> String {
        self.agents_cfg().cfg_of(id).unwrap_or_default().1
    }
    fn resolve_secret(&self, key_ref: &str) -> Result<zeroize::Zeroizing<String>, String> {
        self.secrets().resolve(key_ref, None).map_err(|e| e.to_string())
    }
    fn scene_agent_chat(&self, req: &AgentRunReq) -> Option<Arc<dyn ChatBackend>> {
        let agent = self.agents_cfg().agent(&req.agent)?;
        self.services.b5().scene_agent_chat(Arc::from(agent), req.workspace.clone(), req.model.clone(), req.key.as_ref().map(|k| zeroize::Zeroizing::new((***k).clone())), req.audit.clone(), req.on_usage.clone(), req.exe.clone())
    }

    fn image_models(&self) -> Vec<(String, String)> {
        self.ai().image_models()
    }
    fn picture_choices(&self, found: &[(String, String)]) -> Vec<Cand> {
        self.ai().picture_choices(found)
    }
    fn cpu_only(&self) -> bool {
        !self.agent.ai_profile().local_capable()
    }
    fn gpu_lemonade(&self) -> Option<String> {
        self.services.gpu_lemonade()
    }
    fn lemonade_images_missing(&self) -> Vec<LemonadeModel> {
        self.services.lemonade_images_missing()
    }
    fn lemonade_model(&self, provider: &str, model: &str) -> Option<LemonadeModel> {
        self.services.lemonade_model(provider, model)
    }
    fn provider_is_lemonade(&self, provider: &str) -> bool {
        self.services.provider_is_lemonade(provider)
    }
    fn pull_status(&self) -> Vec<PullStatus> {
        self.services.pull_status()
    }
    fn pull_model(&self, provider: &str, model: &str) -> Result<(), String> {
        self.services.pull_model_for(&self.session.user, provider, model)
    }

    fn host_tts(&self) -> Arc<dyn HostTtsApi> {
        AppCx::host_tts(self)
    }
    fn direct_voice(&self, engine: &SpeechEngine) -> Option<Arc<dyn iyakki_story_model::DirectVoice>> {
        super::direct_voice(self.agent.as_ref(), engine)
    }
    fn speaks_directly(&self, engine: &SpeechEngine) -> bool {
        super::speaks_directly(engine)
    }
    fn can_save_audio(&self, engine: &SpeechEngine) -> bool {
        super::can_save_audio(engine)
    }
    fn player_sink(&self, configured: &str, path: &str) -> Arc<dyn iyakki_story_model::Sink> {
        crate::agent().helpers_b7().command_sink(&self.agent.hostengines_player_command(configured, path).unwrap_or_else(|| configured.to_string()))
    }
    fn reading_synth(&self, opts: SpeakOpts) -> Arc<dyn Synth> {
        self.services.reading_synth(opts)
    }

    fn worker(&self) -> Arc<dyn TalesWorker> {
        Arc::new(CxWorker { svc: self.services.clone(), web: self.web.clone(), policy: self.session.policy.clone(), agent: self.agent.clone() })
    }
}
