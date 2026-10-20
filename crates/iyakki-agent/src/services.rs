//! Runtime hub: owns settings, the vaults, the knowledge base and the model/RAG clients, and hands
//! the apps live views of them. Everything here reads Settings at call time, so changing a model or
//! a backend takes effect immediately without restarting anything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iyakki_proxy::Upstream;

use crate::chat::{ChatBackend, Message, ModelTurn, ToolSpec};
use crate::embed::{HashEmbed, TextEmbedder};
use crate::kb::{Hit, KnowledgeBase};
use crate::providers::{ProviderClient, ProviderCfg};
use crate::rag_ext::ExtClient;
use crate::settings::{ModelChoice, SettingsStore};
use crate::runs::RunStore;
use crate::sessions::SessionStore;
use crate::skills::SkillStore;
use crate::vault::{LocalVault, Secrets, VaultError};
use crate::voice::{self, VoiceSettings};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultState {
    Unlocked,
    Created,
    /// Exists but could not be unlocked (e.g. the password was reset by an admin).
    Locked(String),
    NotApplicable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginReport {
    pub user_vault: VaultState,
    pub system_vault: VaultState,
}

pub struct Services {
    pub settings: Arc<Mutex<SettingsStore>>,
    pub secrets: Secrets,
    pub kb: Arc<Mutex<KnowledgeBase>>,
    /// Recorded agent runs.
    pub runs: Arc<RunStore>,
    pub skills: Arc<SkillStore>,
    /// The internet radio: one player for the whole system, so it keeps playing while apps come and go.
    pub radio: crate::radio::RadioPlayer,
    /// The online radio directory the chat's radio tools search (with an hour of memory).
    pub radio_dir: crate::radiotools::Directory,
    /// The assistant's web client (search, fetch, weather, quotes) and the page each person has open in the browser.
    pub web: crate::webtools::WebClient,
    pub page_snapshot: Mutex<HashMap<String, crate::webtools::PageSnapshot>>,
    /// What the assistant's tools did, kept for a few days.
    pub toollog: crate::toollog::ToolLog,
    /// Detached jobs (long commands that outlive the chat), found again after a restart (see `docs/JOBS.md`).
    pub jobs: crate::jobs::Jobs,
    /// The vendor session id of each chat with an external coding agent (so the next message resumes it).
    pub ext_sessions: crate::extagent::run::SessionMap,
    /// What the last look at each external coding agent found (Settings → Agents, the model list).
    pub ext_status: Arc<crate::extagent::StatusCache>,
    /// Tokens used by each model, for the USAGE tile.
    pub usage: crate::usage::UsageLog,
    /// Each person's events and alarms, shared by the calendar app, the chat and the terminal.
    pub calendar: crate::calendar::CalendarStore,
    /// Command-line tools installed from the CLI tile (kubectl, aws, gcloud, …).
    pub clis: Arc<crate::clis::CliManager>,
    /// The container cluster (k3s) the assistant can run code in.
    pub cluster: Arc<crate::cluster::Cluster>,
    /// The local dev cluster IYAKKI installs and keeps on a bare-metal machine (Settings \u{2192} Dev cluster, SYSTEM).
    pub k3sdev: Arc<crate::k3sdev::K3sDev>,
    /// Machines seen on the local network, and which of them are new.
    pub devwatch: Arc<crate::devwatch::DeviceWatch>,
    /// Each person's notifications and tasks (the INBOX).
    pub inbox: crate::inbox::Inbox,
    /// The speech engines found on this computer (detected in the background, `docs/VOICE.md`) and the browser voice helper.
    pub host_tts: Arc<crate::hostengines::HostTts>,
    watching: std::sync::atomic::AtomicBool,
    /// A local Ollama server, started from the CLI tile once Ollama is installed there.
    pub ollama: Arc<crate::ollama::OllamaServer>,
    /// MCP servers added from the MCP tile.
    pub mcp: crate::mcp::McpStore,
    /// Installed skill packages and the publishers we trust.
    pub packages: Arc<crate::pkg::Packages>,
    pub sessions: Arc<SessionStore>,
    /// Secrets the assistant asked the person to enter themselves (they never pass through the assistant).
    pub secret_requests: crate::secretreq::SecretRequests,
    /// The assistant's use of the person's browser: the job queue the shell services, the questions it asks, the grants and budgets.
    pub browser: crate::browserconsent::BrowserState,
    /// The assistant's `screen_capture`: requests the shell answers with the frame it last drew.
    pub screen: crate::screencap::ScreenCapture,
    /// The assistant's camera tools: the shell's camera, the 5-minute grants and the "in use" sign.
    pub camera: crate::camtools::CameraHub,
    media_indexes: Mutex<HashMap<String, Arc<Mutex<crate::mediaindex::MediaIndex>>>>,
    media_queues: Mutex<HashMap<String, Arc<crate::mediaqueue::DownloadQueue>>>,
    quick: Arc<dyn Upstream>,
    slow: Arc<dyn Upstream>,
    dir: PathBuf,
    user_vaults: Mutex<HashMap<String, LocalVault>>,
    /// The person signed in at the screen: whose home and vault the vendor coding agents are looked at and started with.
    agent_user: Mutex<String>,
    /// For model downloads (their progress arrives over time).
    streamer: Mutex<Option<Arc<dyn iyakki_proxy::Streamer>>>,
    /// For model answers read as they are written (waits long for the first byte: a local model may need loading).
    chat_streamer: Mutex<Option<Arc<dyn iyakki_proxy::Streamer>>>,
    /// How long the last model warm-up took (milliseconds), for the benchmark and the SYSTEM tile.
    pub last_warm_up_ms: std::sync::atomic::AtomicU64,
    pulls: Mutex<HashMap<String, PullStatus>>,
    /// Bringing documents up to date with the embedding model in use: whether it is going and how the last run ended.
    reindexing: std::sync::atomic::AtomicBool,
    reindex_note: Mutex<Option<String>>,
}

/// A model download started from the chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullStatus {
    pub provider: String,
    pub model: String,
    pub status: String,
    pub completed: u64,
    pub total: u64,
    pub done: bool,
    pub error: Option<String>,
}

impl PullStatus {
    pub fn describe(&self) -> String {
        match (&self.error, self.done) {
            (Some(e), _) => format!("{}/{}: failed: {e}", self.provider, self.model),
            (None, true) => format!("{}/{}: downloaded", self.provider, self.model),
            _ if self.total > 0 => format!("{}/{}: {} ({}%, {} of {} MB)", self.provider, self.model, self.status, self.completed * 100 / self.total, self.completed >> 20, self.total >> 20),
            _ => format!("{}/{}: {}", self.provider, self.model, self.status),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionDeleted {
    pub runs: usize,
    pub files: usize,
}

fn count_files(dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten().map(|e| match e.file_type() {
        Ok(t) if t.is_dir() => count_files(&e.path()),
        Ok(_) => 1,
        Err(_) => 0,
    }).sum()
}

fn io_err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl Services {
    /// `quick` is used for tests and model listing (short timeout), `slow` for generation.
    pub fn open(dir: &Path, quick: Arc<dyn Upstream>, slow: Arc<dyn Upstream>) -> Result<Arc<Services>, String> {
        let t0 = std::time::Instant::now();
        std::fs::create_dir_all(dir.join("vault/users")).map_err(io_err)?;
        let settings = SettingsStore::open(&dir.join("settings.json")).map_err(io_err)?;
        let sys_path = dir.join("vault/system.afv");
        let system = if LocalVault::exists(&sys_path) { Some(LocalVault::open(&sys_path).map_err(io_err)?) } else { None };
        let secrets = Secrets::new(system, quick.clone());
        secrets.set_providers(settings.get().secret_providers.clone().into_iter().collect());
        // models a server said can see pictures (Lemonade's vision label) stay known after a restart
        let vision: Vec<String> = settings.get().providers.iter().flat_map(|p| p.model_kinds.iter().filter(|(_, k)| k.as_str() == "vision").map(|(m, _)| m.clone())).collect();
        crate::providers::note_vision_models(&vision);
        let clis = Arc::new(crate::clis::CliManager::new(&dir.join("cli")));
        let svc = Arc::new_cyclic(|weak: &std::sync::Weak<Services>| {
            let embedder: Arc<dyn TextEmbedder> = Arc::new(DynamicEmbed { svc: weak.clone() });
            let kb = Arc::new(Mutex::new(KnowledgeBase::open(&dir.join("kb"), embedder)));
            let skills = Arc::new(SkillStore::new(&dir.join("skills"), kb.clone()));
            Services {
                settings: Arc::new(Mutex::new(settings)),
                secrets,
                packages: Arc::new(crate::pkg::Packages::new(&dir.join("packages"), skills.clone())),
                skills,
                radio: crate::radio::RadioPlayer::default(),
                radio_dir: crate::radiotools::Directory::new(Arc::new(crate::radiotools::UpstreamFetcher(quick.clone()))),
                web: crate::webtools::WebClient::new(),
                page_snapshot: Mutex::new(HashMap::new()),
                toollog: crate::toollog::ToolLog::new(&dir.join("tools.jsonl")),
                jobs: crate::jobs::Jobs::new(&dir.join("jobs")),
                ext_sessions: crate::extagent::run::SessionMap::open(&dir.join("ext-sessions.json")),
                ext_status: Arc::new(crate::extagent::StatusCache::open(&dir.join("ext-models.json"))),
                usage: crate::usage::UsageLog::open(&dir.join("usage.jsonl")),
                calendar: crate::calendar::CalendarStore::new(&dir.join("calendar")),
                clis: clis.clone(),
                cluster: Arc::new(crate::cluster::Cluster::new(&dir.join("cluster"), clis.clone())),
                k3sdev: crate::k3sdev::K3sDev::new(dir, clis.clone(), Arc::new(crate::wsexec::System)),
                devwatch: Arc::new(crate::devwatch::DeviceWatch::open(&dir.join("devices.json"))),
                inbox: crate::inbox::Inbox::new(&dir.join("inbox")),
                host_tts: Arc::new(crate::hostengines::HostTts::new(&[dir.join("voices")])),
                watching: std::sync::atomic::AtomicBool::new(false),
                ollama: {
                    let server = Arc::new(crate::ollama::OllamaServer::new(&dir.join("ollama"), clis.clone()));
                    // when the server answers, it becomes a model provider the chat can use
                    let weak = weak.clone();
                    server.set_on_ready(move |url| {
                        if let Some(svc) = weak.upgrade() {
                            svc.use_local_ollama(url);
                        }
                    });
                    server
                },
                mcp: crate::mcp::McpStore::new(&dir.join("mcp-servers.json")),
                sessions: Arc::new(SessionStore::new(&dir.join("sessions"), kb.clone())),
                kb,
                secret_requests: crate::secretreq::SecretRequests::new(),
                browser: crate::browserconsent::BrowserState::new(),
                screen: crate::screencap::ScreenCapture::new(),
                camera: crate::camtools::CameraHub::new(),
                media_indexes: Mutex::new(HashMap::new()),
                media_queues: Mutex::new(HashMap::new()),
                runs: Arc::new(RunStore::new(&dir.join("runs"))),
                quick,
                slow,
                dir: dir.into(),
                user_vaults: Mutex::new(HashMap::new()),
                agent_user: Mutex::new(String::new()),
                streamer: Mutex::new(None),
                chat_streamer: Mutex::new(None),
                last_warm_up_ms: std::sync::atomic::AtomicU64::new(0),
                pulls: Mutex::new(HashMap::new()),
                reindexing: std::sync::atomic::AtomicBool::new(false),
                reindex_note: Mutex::new(None),
            }
        });
        let t_build = t0.elapsed();
        svc.wire_agent_installs();
        svc.unlock_with_keyfile();
        let t_unlock = t0.elapsed();
        // the managed dev cluster never starts next to the sandbox cluster, and starts with IYAKKI when it was installed here
        {
            let sandbox = svc.cluster.clone();
            svc.k3sdev.set_sandbox_probe(Arc::new(move || matches!(sandbox.state(), crate::cluster::ClusterState::Starting | crate::cluster::ClusterState::Running)));
            svc.k3sdev.autostart();
        }
        log::info!("boot: services opened in {} ms (stores {}, vault key {}, dev cluster {})", t0.elapsed().as_millis(), t_build.as_millis(), (t_unlock - t_build).as_millis(), (t0.elapsed() - t_unlock).as_millis());
        // MP3 files of the MEDIA library play in IYAKKI's own player (the one the RADIO tile uses)
        let weak_svc = Arc::downgrade(&svc);
        crate::media::set_builtin_audio(svc.radio.clone(), Arc::new(move || weak_svc.upgrade().map(|s| s.voice_settings().play_cmd).unwrap_or_default()));
        // a run that ends leaves a note in its owner's inbox
        let weak = Arc::downgrade(&svc);
        svc.runs.set_on_finish(move |log| {
            if let Some(svc) = weak.upgrade() {
                svc.notify_run_finished(log);
            }
        });
        Ok(svc)
    }

    /// Leave a notification in a person's inbox (reminders, finished runs, downloads, warnings). The link says which
    /// app and item it is about. Errors (a full disk, a bad user name) are returned, never raised.
    pub fn notify(&self, user: &str, kind: crate::inbox::Kind, title: &str, body: &str, link: Option<crate::inbox::Link>) -> Result<(), String> {
        let source = link.as_ref().map(|l| l.app.clone()).unwrap_or_else(|| "system".into());
        self.inbox.add(user, kind, title, body, &source, link, crate::clock::now_unix()).map(|_| ())
    }

    /// Like [`Services::notify`], but not again while an unread note with the same title is still there (for warnings
    /// that would otherwise pile up at every sign-in).
    pub fn notify_once(&self, user: &str, kind: crate::inbox::Kind, title: &str, body: &str, link: Option<crate::inbox::Link>) -> Result<(), String> {
        if self.inbox.list(user).iter().any(|n| !n.read && n.title == title) {
            return Ok(());
        }
        self.notify(user, kind, title, body, link)
    }

    fn notify_run_finished(&self, log: &crate::runs::RunLog) {
        use crate::agent::RunEvent;
        let ask: String = log.prompt.chars().take(80).collect();
        let (kind, title, body) = match log.events.last() {
            Some(RunEvent::Final { text }) => (crate::inbox::Kind::Agent, format!("Finished: {ask}"), text.chars().take(400).collect::<String>()),
            Some(RunEvent::Failed { error }) => (crate::inbox::Kind::Error, format!("Failed: {ask}"), error.chars().take(400).collect::<String>()),
            // stopped by the person (or by closing the chat): they know
            _ => return,
        };
        let _ = self.notify(&log.user, kind, &title, &body, Some(crate::inbox::Link::new("runs", &log.id)));
    }

    /// The tool calls waiting for this person's answer, in the runs they have going (from the chat or anywhere else).
    pub fn approvals(&self, user: &str) -> Vec<crate::runs::PendingApproval> {
        self.runs.pending_approvals(Some(user))
    }

    /// Answer one waiting tool call. `approve` false rejects it. Each call is answered on its own; there is no way to
    /// answer several at once.
    pub fn decide_approval(&self, user: &str, run_id: &str, call_id: &str, approve: bool) -> Result<crate::runs::PendingApproval, String> {
        let v = if approve { crate::agent::Verdict::Approve } else { crate::agent::Verdict::Deny };
        self.runs.decide_pending(user, false, run_id, call_id, v)
    }

    /// Re-embed, in the background, every namespace whose documents were embedded by another model than the one in use now
    /// (after the embedding model was changed). One run at a time; each namespace is locked only while it is being done, so
    /// search carries on meanwhile. Returns false when a run is already going or nothing is out of date.
    pub fn reindex_stale_in_background(self: &Arc<Self>) -> bool {
        use std::sync::atomic::Ordering;
        if self.kb.lock().unwrap().stale_namespaces().is_empty() || self.reindexing.swap(true, Ordering::SeqCst) {
            return false;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("iyakki-reindex".into()).spawn(move || {
            let (mut done, mut first_err) = (0usize, None::<String>);
            let stale = me.kb.lock().unwrap().stale_namespaces();
            for ns in stale {
                match me.kb.lock().unwrap().reindex(&ns) {
                    Ok(()) => done += 1,
                    Err(e) => {
                        first_err.get_or_insert(format!("{ns}: {e:?}"));
                    }
                }
            }
            let id = me.kb.lock().unwrap().embedder_id();
            *me.reindex_note.lock().unwrap() = Some(match first_err {
                None => format!("Documents brought up to date with {id} ({done} collection{}).", if done == 1 { "" } else { "s" }),
                Some(e) => format!("Some documents could not be brought up to date with {id}: {e}"),
            });
            me.reindexing.store(false, Ordering::SeqCst);
        });
        if spawned.is_err() {
            self.reindexing.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }

    pub fn is_reindexing(&self) -> bool {
        self.reindexing.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// How the last background run ended (for the Settings page).
    pub fn reindex_note(&self) -> Option<String> {
        self.reindex_note.lock().unwrap().clone()
    }

    /// Make the Ollama server at `url` (the one started from the CLI tile, or one already running on this machine) available
    /// in Models as `ollama-local`, if it is not there already. The chat model is not changed.
    pub fn use_local_ollama(&self, url: &str) {
        let url = url.trim_end_matches('/');
        let mut st = self.settings.lock().unwrap();
        use crate::providers::Kind;
        let known = st.get().providers.iter().any(|p| p.kind == Kind::Ollama && p.base_url.as_deref().map(|u| u.trim_end_matches('/')) == Some(url));
        if known {
            return;
        }
        let mut p = ProviderCfg::new("ollama-local", Kind::Ollama);
        p.base_url = Some(url.to_string());
        let _ = st.upsert_provider(p);
    }

    /// Make the Lemonade server at `base` available in Models as `lemonade-local` (or `lemonade-local-<port>` when that name is
    /// taken by another address). Never overwrites: a provider that already points at this server, or one named like that,
    /// is left exactly as the person set it. Returns what to tell the person.
    pub fn use_local_lemonade(&self, base: &str, version: Option<&str>) -> String {
        use crate::providers::Kind;
        let base = crate::lemonade::api_base(base);
        let found = crate::lemonade::Server::Lemonade { base: base.clone(), version: version.map(str::to_string) }.found();
        let same = |u: &str| crate::lemonade::api_base(u) == base || crate::lemonade::origin(u) == crate::lemonade::origin(&base) && crate::lemonade::api_base(u).ends_with("/v1");
        let mut st = self.settings.lock().unwrap();
        if let Some(p) = st.get().providers.iter().find(|p| p.base_url.as_deref().is_some_and(same)) {
            return format!("{found}; it is already in Models as {}.", p.id);
        }
        let port = base.rsplit_once(':').and_then(|(_, r)| r.split('/').next()).unwrap_or("").to_string();
        let id = ["lemonade-local".to_string(), format!("lemonade-local-{port}")].into_iter().find(|id| st.get().provider(id).is_none() && crate::providers::valid_provider_id(id));
        let Some(id) = id else { return format!("{found}; a provider named lemonade-local already exists, so it was not added (add it in Settings → Models).") };
        let mut p = ProviderCfg::new(&id, Kind::Lemonade);
        p.base_url = Some(base);
        match st.upsert_provider(p) {
            Ok(()) => format!("{found}; added to Models as {id}."),
            Err(e) => format!("{found}, but it could not be added: {e}"),
        }
    }

    /// Look for Ollama and Lemonade on this machine (Ollama's port, Lemonade's ports) and add what answers to Models. Blocking,
    /// with short timeouts; returns one line per server found.
    pub fn detect_local_servers(&self) -> Vec<String> {
        match crate::lemonade::local_ports_override() {
            Some((o, l)) => self.detect_local_servers_at(o, &l),
            None => self.detect_local_servers_at(crate::ollama::DEFAULT_PORT, &crate::lemonade::local_ports()),
        }
    }

    pub fn detect_local_servers_at(&self, ollama_port: u16, lemonade_ports: &[u16]) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(v) = crate::ollama::probe(ollama_port) {
            let url = format!("http://127.0.0.1:{ollama_port}");
            let had = self.settings.lock().unwrap().get().providers.iter().any(|p| p.kind == crate::providers::Kind::Ollama && p.base_url.as_deref().map(|u| u.trim_end_matches('/')) == Some(url.as_str()));
            self.use_local_ollama(&url);
            out.push(format!("Ollama found at {url} (version {v}){}", if had { "; it is already in Models." } else { "; it is in Models as ollama-local." }));
        }
        for port in lemonade_ports {
            if let Some((base, version)) = crate::lemonade::probe_local(*port) {
                let msg = self.use_local_lemonade(&base, version.as_deref());
                let _ = self.refresh_lemonade_at(&base);
                out.push(msg);
            }
        }
        out
    }

    /// Look (briefly, in the background) for a Lemonade server on this machine and add it to Models like Ollama.
    pub fn detect_local_lemonade_in_background(self: &Arc<Self>) {
        let me = self.clone();
        let _ = std::thread::Builder::new().name("iyakki-lemonade-detect".into()).spawn(move || {
            for port in crate::lemonade::local_ports() {
                if let Some((base, version)) = crate::lemonade::probe_local(port) {
                    me.use_local_lemonade(&base, version.as_deref());
                    let _ = me.refresh_lemonade_at(&base);
                }
            }
        });
    }

    fn refresh_lemonade_at(&self, base: &str) -> Result<(), String> {
        let id = self.settings.lock().unwrap().get().providers.iter().find(|p| p.kind == crate::providers::Kind::Lemonade && p.base_url.as_deref().is_some_and(|u| crate::lemonade::api_base(u) == crate::lemonade::api_base(base))).map(|p| p.id.clone());
        id.map_or(Ok(()), |id| self.refresh_lemonade(&id).map(|_| ()))
    }

    /// Which server answers at `url` (Ollama, Lemonade or another OpenAI-compatible one), asked with GET requests only.
    pub fn detect_server(&self, url: &str) -> Result<crate::lemonade::Server, String> {
        crate::lemonade::detect(url, self.quick.as_ref(), &[])
    }

    /// Read a Lemonade provider's typed model list and keep it: the chat models in `models`, the type of each in `model_kinds`.
    pub fn refresh_lemonade(&self, provider: &str) -> Result<Vec<crate::lemonade::Model>, String> {
        let list = self.client_for(provider, true)?.lemonade_catalog()?;
        let mut st = self.settings.lock().unwrap();
        if let Some(mut cfg) = st.get().provider(provider).cloned() {
            cfg.models = list.iter().filter(|m| m.downloaded && m.kind.chats()).map(|m| m.id.clone()).collect();
            cfg.model_kinds = crate::lemonade::kinds_map(&list);
            st.upsert_provider(cfg).map_err(|e| e.to_string())?;
        }
        Ok(list)
    }

    /// A Lemonade provider's typed list without waiting on the network: the one last read, else the saved types.
    pub fn lemonade_cached(&self, provider: &str) -> Vec<crate::lemonade::Model> {
        let Some(cfg) = self.settings.lock().unwrap().get().provider(provider).cloned() else { return vec![] };
        if let Some(c) = cfg.base().ok().and_then(|b| crate::lemonade::cached(&crate::lemonade::api_base(&b))) {
            return c;
        }
        cfg.model_kinds
            .iter()
            .map(|(id, k)| crate::lemonade::Model { id: id.clone(), kind: crate::lemonade::ModelType::from_key(k).unwrap_or(crate::lemonade::ModelType::Other), recipe: String::new(), checkpoint: String::new(), labels: vec![], downloaded: true, loaded: false, suggested: false, size_gb: None, image_defaults: None })
            .collect()
    }

    /// The downloaded models of one type on every enabled Lemonade provider, as (provider, model).
    pub fn lemonade_models(&self, t: crate::lemonade::ModelType) -> Vec<(String, String)> {
        let ids: Vec<String> = self.settings.lock().unwrap().get().providers.iter().filter(|p| p.enabled && p.kind == crate::providers::Kind::Lemonade).map(|p| p.id.clone()).collect();
        ids.into_iter().flat_map(|id| self.lemonade_cached(&id).into_iter().filter(|m| m.kind == t && m.downloaded).map(move |m| (id.clone(), m.id)).collect::<Vec<_>>()).collect()
    }

    /// Load a model into a Lemonade server's memory (or unload it); blocking, for a background job.
    pub fn lemonade_load(&self, provider: &str, model: &str, load: bool) -> Result<String, String> {
        let msg = self.client_for(provider, false)?.lemonade_call(if load { "load" } else { "unload" }, model)?;
        let _ = self.refresh_lemonade(provider);
        Ok(msg)
    }

    /// Read aloud with a Lemonade text-to-speech model (TALES, chat replies): the speech engine becomes its endpoint.
    pub fn use_lemonade_voice(&self, provider: &str, model: &str, voice: Option<&str>) -> Result<String, String> {
        let cfg = self.settings.lock().unwrap().get().provider(provider).cloned().ok_or("unknown provider")?;
        let base = cfg.base()?;
        if !self.speech_server_allowed(&base) {
            return Err(format!("{provider} is {}: what is read aloud would be sent there. Say yes to that first.", crate::speechtime::locality(&base).label()));
        }
        let engine = crate::lemonade::tts_engine(&base, model, voice, cfg.key_ref.clone());
        let label = engine.label();
        self.settings.lock().unwrap().update(|s| s.voice.tts = Some(engine)).map_err(|e| e.to_string())?;
        Ok(format!("Reading voice: {model} on {provider} ({label})."))
    }

    /// Listen with a Lemonade whisper model (Settings → Voice speech-to-text).
    pub fn use_lemonade_stt(&self, provider: &str, model: &str) -> Result<String, String> {
        let cfg = self.settings.lock().unwrap().get().provider(provider).cloned().ok_or("unknown provider")?;
        let base = cfg.base()?;
        if !self.speech_server_allowed(&base) {
            return Err(format!("{provider} is {}: what the microphone hears, and speech whose words are timed, would be sent there. Say yes to that first.", crate::speechtime::locality(&base).label()));
        }
        let engine = crate::lemonade::stt_engine(&base, model, cfg.key_ref.clone());
        self.settings.lock().unwrap().update(|s| s.voice.stt = Some(engine)).map_err(|e| e.to_string())?;
        Ok(format!("Speech-to-text: {model} on {provider}."))
    }

    /// Start reading the network neighbour table every half minute for new devices (once; a second call does nothing).
    pub fn start_device_watch(self: &Arc<Self>) {
        self.start_device_watch_from(std::path::PathBuf::from("/proc/net/arp"), std::time::Duration::from_secs(30));
    }

    pub fn start_device_watch_from(self: &Arc<Self>, arp: std::path::PathBuf, every: std::time::Duration) {
        use std::sync::atomic::Ordering;
        if self.watching.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = Arc::downgrade(self);
        let spawned = std::thread::Builder::new().name("iyakki-device-watch".into()).spawn(move || loop {
            let Some(svc) = me.upgrade() else { return };
            svc.devwatch.observe(&crate::devwatch::read_arp(&arp), crate::clock::now_unix());
            drop(svc);
            std::thread::sleep(every);
        });
        if spawned.is_err() {
            self.watching.store(false, Ordering::SeqCst);
        }
    }

    /// Look (briefly, in the background) for an Ollama already answering on this machine at its usual port, started by the
    /// system or by someone else (on a Mac, the Ollama app), and add it to Models like the one started from the CLI tile.
    pub fn detect_local_ollama_in_background(self: &Arc<Self>) {
        self.detect_local_ollama_at(crate::ollama::DEFAULT_PORT);
    }

    pub fn detect_local_ollama_at(self: &Arc<Self>, port: u16) {
        let me = self.clone();
        let _ = std::thread::Builder::new().name("iyakki-ollama-detect".into()).spawn(move || {
            if crate::ollama::probe(port).is_some() {
                me.use_local_ollama(&format!("http://127.0.0.1:{port}"));
            }
        });
    }

    /// Days the tool log is kept.
    /// The size cap for media downloads, in bytes.
    pub fn media_download_limit(&self) -> u64 {
        match self.settings.lock().unwrap().get().media_download_max_mb {
            0 => crate::mediadl::DEFAULT_MAX_BYTES,
            mb => mb as u64 * (1 << 20),
        }
    }

    pub fn tool_log_days(&self) -> u32 {
        crate::toollog::effective_days(self.settings.lock().unwrap().get().tool_log_days)
    }

    /// A folder of its own for one chat: files the assistant creates for that conversation go here and stay apart
    /// from every other chat's. Made when first needed.
    pub fn session_workdir(&self, user: &str, session_id: &str) -> Result<PathBuf, String> {
        let safe = |s: &str| -> Result<String, String> {
            let t: String = s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
            if t.is_empty() || t != s { Err("bad name for a work folder".into()) } else { Ok(t) }
        };
        let dir = self.dir.join("work").join(safe(user)?).join(safe(session_id)?);
        std::fs::create_dir_all(&dir).map_err(io_err)?;
        std::fs::canonicalize(&dir).map_err(io_err)
    }

    /// The streaming transport, when one was set (downloads use it).
    pub fn streamer(&self) -> Option<Arc<dyn iyakki_proxy::Streamer>> {
        self.streamer.lock().unwrap().clone()
    }

    /// The transport model answers are streamed over; without one answers arrive in one piece.
    pub fn set_chat_streamer(&self, s: Arc<dyn iyakki_proxy::Streamer>) {
        *self.chat_streamer.lock().unwrap() = Some(s);
    }

    pub fn chat_streamer(&self) -> Option<Arc<dyn iyakki_proxy::Streamer>> {
        self.chat_streamer.lock().unwrap().clone()
    }

    /// Load the chat model into memory in the background when it is a local Ollama model, so the first question is not slow.
    pub fn warm_up_chat(self: &Arc<Self>) {
        let svc = self.clone();
        let _ = std::thread::Builder::new().name("iyakki-warm".into()).spawn(move || {
            let Some(c) = svc.settings.lock().unwrap().get().chat.clone() else { return };
            if let Ok(client) = svc.client_for(&c.provider, false) {
                if let Ok(t) = client.warm_up(&c.model) {
                    svc.last_warm_up_ms.store(t, std::sync::atomic::Ordering::Relaxed);
                }
            }
        });
    }

    pub fn set_streamer(&self, s: Arc<dyn iyakki_proxy::Streamer>) {
        self.clis.set_streamer(s.clone());
        *self.streamer.lock().unwrap() = Some(s);
    }

    /// Every model of every enabled provider, as (provider, model); the server's own list when it answers, otherwise
    /// the one saved in Settings.
    pub fn all_models(&self) -> Vec<(String, String)> {
        let providers: Vec<ProviderCfg> = self.settings.lock().unwrap().get().providers.iter().filter(|p| p.enabled).cloned().collect();
        let mut out = Vec::new();
        for p in providers {
            let live = self.client_for(&p.id, true).and_then(|c| c.list_models());
            for m in live.unwrap_or_else(|_| p.models.clone()) {
                out.push((p.id.clone(), m));
            }
        }
        out
    }

    /// The models that draw pictures, as (provider, model).
    pub fn image_models(&self) -> Vec<(String, String)> {
        // a Lemonade server says which of its models draw (they are not in its chat list); the others are judged by name
        let mut v: Vec<(String, String)> = self.all_models().into_iter().filter(|(_, m)| crate::models::is_image_model(m)).collect();
        for m in self.lemonade_models(crate::lemonade::ModelType::Image) {
            if !v.contains(&m) {
                v.push(m);
            }
        }
        v
    }

    /// Draws a picture with `model` (or, when none is named, the first picture model found).
    pub fn generate_image(&self, model: Option<(&str, &str)>, prompt: &str, size: &str) -> Result<(String, String, Vec<u8>, &'static str), String> {
        let (provider, model) = match model {
            Some((p, m)) => (p.to_string(), m.to_string()),
            None => {
                let found = self.image_models();
                let first = found.first().cloned().ok_or_else(|| {
                    let checked: Vec<String> = self.settings.lock().unwrap().get().providers.iter().filter(|p| p.enabled).map(|p| p.id.clone()).collect();
                    format!("No model that draws pictures was found{}. A picture model such as Qwen-Image, FLUX or Stable Diffusion has to be available on a connected server; I can download one into Ollama if you tell me its name.", if checked.is_empty() { " (no model provider is connected at all)".to_string() } else { format!(" (looked at: {})", checked.join(", ")) })
                })?;
                first
            }
        };
        let (bytes, ext) = self.client_for(&provider, false)?.generate_image(&model, prompt, size)?;
        Ok((provider, model, bytes, ext))
    }

    pub fn pull_status(&self) -> Vec<PullStatus> {
        let mut v: Vec<PullStatus> = self.pulls.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| (&a.provider, &a.model).cmp(&(&b.provider, &b.model)));
        v
    }

    /// Starts downloading `model` into the Ollama server `provider` and returns at once; `pull_status` follows it.
    pub fn pull_model(self: &Arc<Self>, provider: &str, model: &str) -> Result<(), String> {
        if !crate::models::valid_pull_name(model) {
            return Err("that is not a model name Ollama can download".into());
        }
        let client = self.client_for(provider, false)?;
        let (url, headers, body) = client.pull_request(model)?;
        let streamer = self.streamer.lock().unwrap().clone().ok_or("no network connection is set up for downloads")?;
        let key = format!("{provider}/{model}");
        {
            let mut p = self.pulls.lock().unwrap();
            if p.get(&key).is_some_and(|s| !s.done && s.error.is_none()) {
                return Err(format!("{model} is already downloading"));
            }
            p.insert(key.clone(), PullStatus { provider: provider.into(), model: model.into(), status: "starting".into(), completed: 0, total: 0, done: false, error: None });
        }
        let (me, provider, model) = (self.clone(), provider.to_string(), model.to_string());
        std::thread::Builder::new().name("iyakki-pull".into()).spawn(move || me.run_pull(&key, &provider, &model, &*streamer, &url, &headers, &body)).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn run_pull(&self, key: &str, provider: &str, model: &str, streamer: &dyn iyakki_proxy::Streamer, url: &str, headers: &[(String, String)], body: &[u8]) {
        use std::io::BufRead;
        let set = |f: &dyn Fn(&mut PullStatus)| {
            if let Some(s) = self.pulls.lock().unwrap().get_mut(key) {
                f(s);
            }
        };
        let lemonade = self.settings.lock().unwrap().get().provider(provider).is_some_and(|p| p.kind == crate::providers::Kind::Lemonade);
        let fail = |e: String| set(&|s: &mut PullStatus| s.error = Some(if lemonade { format!("the download of `{model}` into Lemonade failed: {e}") } else { crate::models::explain_pull_error(model, &e) }));
        let head = match streamer.post(url, headers, body) {
            Ok(h) => h,
            Err(e) => return fail(format!("could not reach the server: {e}")),
        };
        let status = head.status;
        let mut reader = std::io::BufReader::new(head.body);
        let mut line = String::new();
        let mut finished = false;
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            match crate::models::pull_line(&line) {
                Some(crate::models::PullLine::Progress { status, completed, total }) => set(&|s: &mut PullStatus| {
                    s.status = status.clone();
                    if total > 0 {
                        s.completed = completed;
                        s.total = total;
                    }
                }),
                Some(crate::models::PullLine::Done) => {
                    finished = true;
                    break;
                }
                Some(crate::models::PullLine::Error(e)) => return fail(e),
                None => {
                    if status >= 400 {
                        return fail(format!("HTTP {status}: {}", line.trim()));
                    }
                }
            }
        }
        if !finished {
            return fail(if status >= 400 { format!("HTTP {status}") } else { "the server stopped answering before the download finished".into() });
        }
        // the new model goes into the provider's list so it can be chosen and found
        let lemonade = self.settings.lock().unwrap().get().provider(provider).is_some_and(|p| p.kind == crate::providers::Kind::Lemonade);
        if lemonade {
            let _ = self.refresh_lemonade(provider);
        } else if let Ok(names) = self.client_for(provider, true).and_then(|c| c.list_models()) {
            let mut st = self.settings.lock().unwrap();
            if let Some(mut cfg) = st.get().provider(provider).cloned() {
                cfg.models = names;
                let _ = st.upsert_provider(cfg);
            }
        }
        set(&|s: &mut PullStatus| {
            s.done = true;
            s.status = "success".into();
        });
    }

    /// Deletes a chat completely: its transcript, its memory, its agent runs and its work folder with every file in
    /// it. Returns what was removed, for the confirmation message.
    pub fn delete_session(&self, user: &str, id: &str) -> Result<SessionDeleted, String> {
        self.sessions.delete(user, id)?;
        let runs = self.runs.delete_for_session(user, id);
        let dir = self.dir.join("work").join(user).join(id);
        let files = count_files(&dir);
        if dir.is_dir() {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("the chat was deleted but its folder could not be removed: {e}"))?;
        }
        Ok(SessionDeleted { runs, files })
    }

    /// How many files a chat's work folder holds (for the question "delete its files too?").
    pub fn session_files(&self, user: &str, id: &str) -> usize {
        count_files(&self.dir.join("work").join(user).join(id))
    }

    /// The upstream for long requests (speech, transcription).
    pub fn slow_upstream(&self) -> Arc<dyn Upstream> {
        self.slow.clone()
    }

    pub fn quick_upstream(&self) -> Arc<dyn Upstream> {
        self.quick.clone()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn system_vault_state(&self) -> (bool, bool) {
        self.system_state()
    }

    fn system_path(&self) -> PathBuf {
        self.dir.join("vault/system.afv")
    }

    fn user_path(&self, user: &str) -> PathBuf {
        self.dir.join(format!("vault/users/{user}.afv"))
    }

    // ------------------------------------------------------------ vault lifecycle

    /// Called with the password the user just typed. Opens their personal vault (creating it the
    /// first time) and, for admins, the system vault.
    pub fn on_login(&self, user: &str, password: &str, is_admin: bool) -> LoginReport {
        *self.agent_user.lock().unwrap_or_else(|e| e.into_inner()) = user.to_string();
        let slot = format!("user:{user}");
        let up = self.user_path(user);
        let user_state = if LocalVault::exists(&up) {
            match LocalVault::open(&up).and_then(|mut v| v.unlock(&slot, password).map(|_| v)) {
                Ok(v) => {
                    self.user_vaults.lock().unwrap().insert(user.into(), v);
                    VaultState::Unlocked
                }
                Err(e) => VaultState::Locked(e.to_string()),
            }
        } else {
            match LocalVault::create(&up, &slot, password) {
                Ok(v) => {
                    self.user_vaults.lock().unwrap().insert(user.into(), v);
                    VaultState::Created
                }
                Err(e) => VaultState::Locked(e.to_string()),
            }
        };
        let sys_state = if is_admin { self.admin_login_system(&slot, password) } else { VaultState::NotApplicable };
        LoginReport { user_vault: user_state, system_vault: sys_state }
    }

    fn admin_login_system(&self, slot: &str, password: &str) -> VaultState {
        let mut g = self.secrets.system.lock().unwrap();
        if g.is_none() {
            let p = self.system_path();
            *g = if LocalVault::exists(&p) { LocalVault::open(&p).ok() } else { LocalVault::create(&p, slot, password).ok() };
            return match g.as_ref() {
                Some(v) if v.is_unlocked() => VaultState::Created,
                Some(_) => VaultState::Locked("could not read the system vault".into()),
                None => VaultState::Locked("could not create the system vault".into()),
            };
        }
        let v = g.as_mut().expect("checked");
        if v.is_unlocked() {
            // Another admin already opened it: give this admin their own slot if they lack one.
            if !v.has_slot(slot) {
                let _ = v.add_slot(slot, password);
            }
            return VaultState::Unlocked;
        }
        if !v.has_slot(slot) {
            return VaultState::Locked("this admin has no key slot; an admin with access must sign in first".into());
        }
        match v.unlock(slot, password) {
            Ok(()) => VaultState::Unlocked,
            Err(e) => VaultState::Locked(e.to_string()),
        }
    }

    pub fn on_logout(&self, user: &str, is_admin: bool) {
        self.user_vaults.lock().unwrap().remove(user);
        self.page_snapshot.lock().unwrap().remove(user);
        crate::datatools::forget(user);
        self.browser.on_logout(user);
        if is_admin && self.settings.lock().unwrap().get().vault.lock_on_admin_logout {
            if let Some(v) = self.secrets.system.lock().unwrap().as_mut() {
                v.lock();
            }
        }
    }

    /// Keep vault slots in step when the user changes their own password.
    pub fn on_password_change(&self, user: &str, old: &str, new: &str) {
        let slot = format!("user:{user}");
        if let Ok(mut v) = LocalVault::open(&self.user_path(user)) {
            let _ = v.change_password(&slot, old, new);
            self.user_vaults.lock().unwrap().remove(user);
        }
        if let Some(v) = self.secrets.system.lock().unwrap().as_mut() {
            if v.has_slot(&slot) {
                let _ = v.change_password(&slot, old, new);
            }
        }
    }

    /// An admin reset someone's password: their personal vault can no longer be opened (by design,
    /// admins cannot read user secrets). Start a fresh empty one at next login by removing the old.
    pub fn reset_user_vault(&self, user: &str) -> Result<(), String> {
        self.user_vaults.lock().unwrap().remove(user);
        match std::fs::remove_file(self.user_path(user)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(e)),
        }
    }

    pub fn unlock_with_keyfile(&self) -> bool {
        let Some(path) = self.settings.lock().unwrap().get().vault.keyfile.clone() else { return false };
        let Ok(key) = std::fs::read_to_string(&path) else { return false };
        let mut g = self.secrets.system.lock().unwrap();
        match g.as_mut() {
            Some(v) if v.has_slot("keyfile") => v.unlock("keyfile", key.trim()).is_ok(),
            _ => false,
        }
    }

    /// Create a random key file and add a `keyfile` slot so the system vault can open unattended.
    pub fn enable_keyfile(&self, path: &str) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut raw = [0u8; 32];
        iyakki_sec::random_fill(&mut raw).map_err(|e| io_err(e.to_string()))?;
        let key: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let mut g = self.secrets.system.lock().unwrap();
        let v = g.as_mut().ok_or("the system vault does not exist yet")?;
        if !v.is_unlocked() {
            return Err(VaultError::Locked.to_string());
        }
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(|e| format!("{path}: {e}"))?;
        f.write_all(key.as_bytes()).map_err(io_err)?;
        v.add_slot("keyfile", &key).map_err(|e| {
            let _ = std::fs::remove_file(path);
            e.to_string()
        })?;
        drop(g);
        self.settings.lock().unwrap().update(|s| s.vault.keyfile = Some(path.into())).map_err(io_err)
    }

    fn system_state(&self) -> (bool, bool) {
        let g = self.secrets.system.lock().unwrap();
        (g.is_some(), g.as_ref().is_some_and(|v| v.is_unlocked()))
    }

    pub fn system_slots(&self) -> Vec<String> {
        self.secrets.system.lock().unwrap().as_ref().map(|v| v.slot_ids()).unwrap_or_default()
    }

    pub fn lock_system(&self) {
        if let Some(v) = self.secrets.system.lock().unwrap().as_mut() {
            v.lock();
        }
    }

    // secret CRUD — callers enforce who may use which (system: manage_settings; user: own only)
    pub fn system_names(&self) -> Result<Vec<String>, VaultError> {
        self.secrets.system.lock().unwrap().as_ref().ok_or(VaultError::Locked)?.names()
    }
    pub fn system_set(&self, name: &str, value: &str) -> Result<(), VaultError> {
        self.secrets.system.lock().unwrap().as_mut().ok_or(VaultError::Locked)?.set(name, value)
    }
    pub fn system_delete(&self, name: &str) -> Result<(), VaultError> {
        self.secrets.system.lock().unwrap().as_mut().ok_or(VaultError::Locked)?.delete(name)
    }
    pub fn user_names(&self, user: &str) -> Result<Vec<String>, VaultError> {
        self.user_vaults.lock().unwrap().get(user).ok_or(VaultError::Locked)?.names()
    }
    pub fn user_set(&self, user: &str, name: &str, value: &str) -> Result<(), VaultError> {
        self.user_vaults.lock().unwrap().get_mut(user).ok_or(VaultError::Locked)?.set(name, value)
    }
    pub fn user_get(&self, user: &str, name: &str) -> Result<zeroize::Zeroizing<String>, VaultError> {
        self.user_vaults.lock().unwrap().get(user).ok_or(VaultError::Locked)?.get(name)
    }
    pub fn user_delete(&self, user: &str, name: &str) -> Result<(), VaultError> {
        self.user_vaults.lock().unwrap().get_mut(user).ok_or(VaultError::Locked)?.delete(name)
    }
    /// The person's answer to a secret request: the value goes into their vault under `name`, and the waiting tool is told
    /// only the name. Returns the name stored.
    pub fn answer_secret_request(&self, user: &str, id: &str, name: &str, value: &str) -> Result<String, String> {
        if self.secret_requests.get(user, id).is_none() {
            return Err("that request is not waiting any more".into());
        }
        let name = name.trim();
        if !crate::vault::valid_secret_name(name) {
            return Err("a vault name is lowercase letters, digits, . _ - (up to 64), for example svc.nas".into());
        }
        if value.is_empty() {
            return Err("type the secret first".into());
        }
        self.user_set(user, name, value).map_err(|e| match e {
            VaultError::Locked => "your personal vault is locked (it could not be opened at sign-in)".to_string(),
            other => other.to_string(),
        })?;
        self.secret_requests.resolve(user, id, crate::secretreq::Outcome::Stored(name.to_string()))?;
        Ok(name.to_string())
    }

    pub fn cancel_secret_request(&self, user: &str, id: &str) -> Result<(), String> {
        self.secret_requests.resolve(user, id, crate::secretreq::Outcome::Cancelled)
    }

    /// The person's media index, shared by the MEDIA app and the assistant's tools.
    pub fn media_index(self: &Arc<Self>, user: &str) -> Arc<Mutex<crate::mediaindex::MediaIndex>> {
        let mut g = self.media_indexes.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(user.to_string()).or_insert_with(|| Arc::new(Mutex::new(crate::mediaindex::MediaIndex::open(&self.dir, user, Some(self.embedder()))))).clone()
    }

    /// The person's download queue, shared by the MEDIA app and the assistant's tools.
    pub fn media_queue(&self, user: &str) -> Arc<crate::mediaqueue::DownloadQueue> {
        let mut g = self.media_queues.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(user.to_string()).or_insert_with(|| Arc::new(crate::mediaqueue::DownloadQueue::open(&self.dir, user))).clone()
    }

    pub fn user_vault_unlocked(&self, user: &str) -> bool {
        self.user_vaults.lock().unwrap().contains_key(user)
    }

    // ------------------------------------------------------------ models

    /// Push secret-provider settings into the resolver (call after editing them).
    pub fn refresh_secret_providers(&self) {
        let p = self.settings.lock().unwrap().get().secret_providers.clone();
        self.secrets.set_providers(p.into_iter().collect());
    }

    pub fn client_for(&self, provider_id: &str, quick: bool) -> Result<ProviderClient, String> {
        let cfg: ProviderCfg = self.settings.lock().unwrap().get().provider(provider_id).cloned().ok_or_else(|| format!("unknown provider `{provider_id}`"))?;
        Ok(ProviderClient::new(cfg, self.secrets.clone(), if quick { self.quick.clone() } else { self.slow.clone() }))
    }

    /// The external-agent settings, plus the CLI tile's folder so a Claude Code or Copilot installed from the tile is found.
    pub fn agents_cfg(&self) -> crate::extagent::AgentsCfg {
        let mut c = self.settings.lock().unwrap_or_else(|e| e.into_inner()).get().agents.clone();
        c.extra_bin = Some(self.clis.bin_dir());
        // the person's own home (the one the login runs in) and the token kept in their vault: one place for the chip, the status
        // lookups, the agents' runs and the smoke tests
        let user = self.agent_user.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if !user.is_empty() {
            let home = crate::extagent::login::login_home(&user, &self.dir);
            c.env = vec![("HOME".into(), home.display().to_string()), ("CLAUDE_CONFIG_DIR".into(), home.join(".claude").display().to_string())];
            match self.user_get(&user, crate::extagent::login::COPILOT_VAULT) {
                Ok(v) => {
                    if let Some((login, _)) = crate::extagent::login::parse_vault_entry(&v) {
                        c.vault_logins.insert(crate::extagent::COPILOT.into(), if login.is_empty() { "GitHub".into() } else { login });
                    }
                }
                Err(VaultError::Locked) => c.vault_locked = true,
                Err(_) => {}
            }
        }
        c
    }

    /// The token of a coding agent kept in the person's vault, for the one process started for them (never for this process's own
    /// environment): `(variable, token)`.
    pub fn agent_vault_token(&self, user: &str, agent: &str) -> Option<(&'static str, zeroize::Zeroizing<String>)> {
        if agent != crate::extagent::COPILOT {
            return None;
        }
        let v = self.user_get(user, crate::extagent::login::COPILOT_VAULT).ok()?;
        crate::extagent::login::parse_vault_entry(&v).map(|(_, t)| (crate::extagent::login::COPILOT_TOKEN_ENV, t))
    }

    /// An install from the CLI tile finished: look at the external agents again at once (no restart).
    pub fn wire_agent_installs(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        self.clis.set_on_installed(Arc::new(move |_id| {
            if let Some(s) = me.upgrade() {
                s.ext_status.refresh(s.agents_cfg());
            }
        }));
    }

    /// A chat backend that always follows the *current* Settings.
    pub fn chat_backend(self: &Arc<Self>) -> Arc<dyn ChatBackend> {
        Arc::new(DynamicChat { svc: self.clone() })
    }

    /// A chat backend that always uses `choice` (a model chosen for one tile, such as TALES) instead of the chat model.
    /// Providers are looked up when a request is made, so a provider removed later gives an error, not a stale answer.
    pub fn chat_backend_for(self: &Arc<Self>, choice: ModelChoice) -> Arc<dyn ChatBackend> {
        Arc::new(FixedChat { svc: self.clone(), choice })
    }

    pub fn embedder(self: &Arc<Self>) -> Arc<dyn TextEmbedder> {
        Arc::new(DynamicEmbed { svc: Arc::downgrade(self) })
    }

    // ------------------------------------------------------------ voice

    pub fn voice_settings(&self) -> VoiceSettings {
        self.settings.lock().unwrap().get().voice.clone()
    }

    // ------------------------------------------------------------ sound output (Settings -> Sound)

    pub fn sound_settings(&self) -> crate::audioout::SoundSettings {
        self.settings.lock().unwrap().get().sound.clone()
    }

    /// Change Settings -> Sound (callers check that the person may). Takes effect for the next sound, and at once for the
    /// radio's volume.
    /// The volume is remembered for the output in use (restored when that output is used again, see `audioout`); the
    /// settings returned are the ones in effect afterwards (another output chosen brings its own volume).
    pub fn update_sound(&self, f: impl FnOnce(&mut crate::audioout::SoundSettings)) -> Result<crate::audioout::SoundSettings, String> {
        let in_use = crate::audioout::global().current_key();
        let now = {
            let mut st = self.settings.lock().unwrap();
            st.update(|s| {
                f(&mut s.sound);
                if let Some(k) = &in_use {
                    s.sound.remember_level(k);
                }
            })
            .map_err(|e| e.to_string())?;
            st.get().sound.clone()
        };
        // the settings lock is released: the selector may save a restored volume through `start_sound_output`'s hook
        crate::audioout::apply_prefs(now);
        Ok(self.sound_settings())
    }

    /// The real front end: IYAKKI's players use the output of Settings -> Sound from now on, and watch for changes. When the
    /// selector restores the remembered volume of an output, the settings file follows.
    pub fn start_sound_output(&self) {
        let store = Arc::downgrade(&self.settings);
        crate::audioout::global().set_on_prefs(move |p| {
            if let Some(store) = store.upgrade() {
                if let Ok(mut st) = store.lock() {
                    let v = p.volume_pct;
                    if let Err(e) = st.update(|s| s.sound.volume_pct = v) {
                        log::warn!("sound: could not save the volume of the output now in use: {e}");
                    }
                }
            }
        });
        crate::audioout::install(self.sound_settings());
    }

    /// Test sound: a short gentle chime on `device` (an ALSA device such as `plughw:0,3`), else on the output in use. Waits
    /// for the end (up to 5 s); says where it played.
    pub fn test_sound(&self, device: Option<&str>) -> Result<String, String> {
        if self.sound_settings().muted {
            return Err("Sound is muted in Settings → Sound.".into());
        }
        let play = self.voice_settings().play_cmd;
        let cmd = match device {
            Some(d) => crate::audioout::command_with_device(&play, d)?,
            None => play,
        };
        let out = crate::audioout::global();
        let args = crate::audioout::route_args(voice::split_command(&cmd)?, out.device().as_deref());
        let used = args.iter().position(|a| a == "-D").and_then(|i| args.get(i + 1).cloned());
        if args.is_empty() {
            return Err("the play command is empty (Settings → Voice)".into());
        }
        // IYAKKI's own device carries the volume itself; a device named by hand (one output's row) bypasses it
        let wav = crate::audioout::scale_wav(&crate::audioout::test_tone_wav(), crate::audioout::gain_for(out, &args));
        voice::run_capture(voice::command_for(&args, None), Some(&wav), std::time::Duration::from_secs(8)).map_err(|e| format!("The test sound did not play on {}: {e}", used.as_deref().unwrap_or("the default output")))?;
        Ok(match used {
            Some(d) if d == crate::softvol::PCM => format!("Played a short chime on the output in use (ALSA device {d}, with IYAKKI's volume)."),
            Some(d) => format!("Played a short chime on {d}."),
            None => "Played a short chime on the default output.".into(),
        })
    }

    /// Speech to text for recorded 16 kHz mono samples.
    pub fn transcribe_samples(&self, pcm: &[i16]) -> Result<String, String> {
        let engine = self.voice_settings().stt.ok_or("No speech-to-text engine is set up. Open Settings → Voice.")?;
        if voice::peak(pcm) < 150 {
            return Err("I only heard silence. Check the microphone and its volume.".into());
        }
        let text = voice::transcribe(&engine, &voice::encode_wav(pcm, voice::SAMPLE_RATE), &self.secrets, self.slow.as_ref())?;
        if text.is_empty() { Err("No speech was recognised.".into()) } else { Ok(text) }
    }

    /// The engine that makes speech for the assistant and the stories: the one of Settings → Voice, else the best engine found
    /// on this computer (`hostengines`; `lang` prefers one that has a voice for it). The browser voice is not here: it needs
    /// the screen (see `story::effective_engine_auto`).
    pub fn effective_tts(&self, lang: Option<&str>) -> Option<voice::SpeechEngine> {
        self.host_tts.ensure(); // the first call starts the look for engines in the background
        // a Lemonade voice (Kokoro) on this computer, or on a server the person allowed, reads better than the engines found here
        self.voice_settings().tts.or_else(|| self.lemonade_auto_voice(lang)).or_else(|| self.host_tts.auto_engine(lang))
    }

    /// Text to speech; returns WAV bytes.
    pub fn synthesize_text(&self, text: &str) -> Result<Vec<u8>, String> {
        let engine = self.effective_tts(None).ok_or("No text-to-speech engine is set up. Open Settings → Voice.")?;
        voice::synthesize(&engine, text, &self.secrets, self.slow.as_ref())
    }

    /// Text to speech with a voice and speed chosen for this request (the STORY tile).
    pub fn synthesize_with_opts(&self, text: &str, opts: &voice::SpeakOpts) -> Result<Vec<u8>, String> {
        let engine = self.effective_tts(opts.lang.as_deref()).ok_or("No text-to-speech engine is set up. Open Settings → Voice.")?;
        voice::synthesize_with(&engine, text, opts, &self.secrets, self.slow.as_ref())
    }

    /// Start reading `text` aloud (a chat reply): the engine speaks it itself when it can, otherwise a WAV is made and played
    /// by the play command. Stop it with `Player::stop`.
    pub fn speak_player(&self, text: &str) -> Result<voice::Player, String> {
        let v = self.voice_settings();
        let engine = self.effective_tts(None).ok_or("No text-to-speech engine is set up. Open Settings → Voice.")?;
        if let voice::SpeechEngine::Host { engine: id, program, voice: vo } = &engine {
            let said = voice::speakable(text);
            if said.is_empty() {
                return Err("nothing to say".into());
            }
            let spec = crate::hostengines::Spec::from_parts(id, program.as_deref(), vo.as_deref())?;
            return crate::hostengines::speak(&spec, &said, None, v.speed_pct as f32 / 100.0, &v.play_cmd);
        }
        let wav = voice::synthesize(&engine, text, &self.secrets, self.slow.as_ref())?;
        voice::Player::start(&v.play_cmd, &wav)
    }

    // ------------------------------------------------------------ RAG

    /// Search the local knowledge base and every enabled external backend, merging the ranked
    /// lists with Reciprocal Rank Fusion. Backend failures become warnings, not errors.
    pub fn search(&self, namespaces: &[&str], query: &str, k: usize) -> (Vec<Hit>, Vec<String>) {
        let mut lists = vec![self.kb.lock().unwrap().search(namespaces, query, k * 2)];
        let mut warnings = Vec::new();
        let (exts, use_ext) = {
            let s = self.settings.lock().unwrap();
            (s.get().external_rag.iter().filter(|e| e.enabled).map(|e| e.backend.clone()).collect::<Vec<_>>(), s.get().rag.use_external)
        };
        if use_ext {
            let qvec = if exts.iter().any(|e| e.needs_vector()) {
                match DynamicEmbed::embed_with(self, &[query.to_string()]) {
                    Ok(mut v) => v.pop(),
                    Err(e) => {
                        warnings.push(format!("query embedding failed: {e}"));
                        None
                    }
                }
            } else {
                None
            };
            for e in exts {
                let id = e.id().to_string();
                let c = ExtClient::new(e, self.secrets.clone(), self.quick.clone());
                match c.search(query, qvec.as_deref(), k * 2) {
                    Ok(h) => lists.push(h),
                    Err(m) => warnings.push(format!("{id}: {m}")),
                }
            }
        }
        (fuse(lists, k), warnings)
    }
}

/// Reciprocal Rank Fusion across several ranked hit lists (identical text counts once).
pub fn fuse(lists: Vec<Vec<Hit>>, k: usize) -> Vec<Hit> {
    let mut scored: Vec<(f32, Hit)> = Vec::new();
    for list in lists {
        for (rank, h) in list.into_iter().enumerate() {
            let add = 1.0 / (60.0 + rank as f32 + 1.0);
            match scored.iter_mut().find(|(_, x)| x.text == h.text) {
                Some((s, _)) => *s += add,
                None => scored.push((add, h)),
            }
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().take(k).map(|(s, mut h)| {
        h.score = s;
        h
    }).collect()
}

struct DynamicChat {
    svc: Arc<Services>,
}

struct FixedChat {
    svc: Arc<Services>,
    choice: ModelChoice,
}

impl ChatBackend for FixedChat {
    fn complete(&self, history: &[Message]) -> Result<String, String> {
        self.svc.client_for(&self.choice.provider, false)?.chat(&self.choice.model, history)
    }

    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.svc.client_for(&self.choice.provider, false)?.chat_turn(&self.choice.model, history, tools)
    }

    fn complete_tools_stream(&self, history: &[Message], tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        let client = self.svc.client_for(&self.choice.provider, false)?;
        match self.svc.chat_streamer() {
            Some(s) => client.chat_turn_stream(&self.choice.model, history, tools, s.as_ref(), on_text),
            None => client.chat_turn(&self.choice.model, history, tools),
        }
    }
}

impl ChatBackend for DynamicChat {
    fn complete(&self, history: &[Message]) -> Result<String, String> {
        let choice: Option<ModelChoice> = self.svc.settings.lock().unwrap().get().chat.clone();
        let c = choice.ok_or("No chat model selected. Open Settings → Models, add a provider and pick a model.")?;
        self.svc.client_for(&c.provider, false)?.chat(&c.model, history)
    }

    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        let choice: Option<ModelChoice> = self.svc.settings.lock().unwrap().get().chat.clone();
        let c = choice.ok_or("No chat model selected. Open Settings → Models, add a provider and pick a model.")?;
        self.svc.client_for(&c.provider, false)?.chat_turn(&c.model, history, tools)
    }

    fn complete_tools_stream(&self, history: &[Message], tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        let choice: Option<ModelChoice> = self.svc.settings.lock().unwrap().get().chat.clone();
        let c = choice.ok_or("No chat model selected. Open Settings → Models, add a provider and pick a model.")?;
        let client = self.svc.client_for(&c.provider, false)?;
        match self.svc.chat_streamer() {
            Some(s) => client.chat_turn_stream(&c.model, history, tools, s.as_ref(), on_text),
            None => client.chat_turn(&c.model, history, tools),
        }
    }
}

/// Embeddings that follow Settings: the configured provider+model, else the built-in hashing embedder.
pub struct DynamicEmbed {
    svc: std::sync::Weak<Services>,
}

impl DynamicEmbed {
    fn embed_with(svc: &Services, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let choice = svc.settings.lock().unwrap().get().embedding.clone();
        match choice {
            None => HashEmbed::default().embed(texts),
            Some(c) => {
                let client = svc.client_for(&c.provider, false)?;
                let mut out = Vec::with_capacity(texts.len());
                for batch in texts.chunks(64) {
                    out.extend(client.embed(&c.model, batch)?);
                }
                Ok(out)
            }
        }
    }
}

impl TextEmbedder for DynamicEmbed {
    fn id(&self) -> String {
        match self.svc.upgrade() {
            Some(s) => match s.settings.lock().unwrap().get().embedding.clone() {
                Some(c) => format!("remote:{}:{}", c.provider, c.model),
                None => HashEmbed::default().id(),
            },
            None => HashEmbed::default().id(),
        }
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        match self.svc.upgrade() {
            Some(s) => Self::embed_with(&s, texts),
            None => HashEmbed::default().embed(texts),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Kind;
    use crate::rag_ext::ExternalRag;
    use crate::settings::ExtEntry;
    use iyakki_proxy::UpstreamResponse;
    use std::collections::BTreeMap;

    #[test]
    fn deleting_a_chat_removes_its_transcript_memory_runs_and_work_folder_and_nothing_else() {
        let (s, _m, _d) = svc("delsess", |_, _| (200, String::new()));
        let (mut one, mut two) = (s.sessions.create("alice").unwrap(), s.sessions.create("alice").unwrap());
        s.sessions.save(&mut one).unwrap();
        s.sessions.save(&mut two).unwrap();
        s.sessions.remember_turn(&one, "what is x", "x is 5");
        let (w1, w2) = (s.session_workdir("alice", &one.id).unwrap(), s.session_workdir("alice", &two.id).unwrap());
        std::fs::create_dir_all(w1.join("sub")).unwrap();
        std::fs::write(w1.join("a.txt"), "a").unwrap();
        std::fs::write(w1.join("sub/b.txt"), "b").unwrap();
        std::fs::write(w2.join("keep.txt"), "k").unwrap();
        let log = |id: &str, session: Option<&str>| crate::runs::RunLog { id: id.into(), user: "alice".into(), started: 1, prompt: "p".into(), status: "completed".into(), events: vec![], session: session.map(String::from), task_id: None, row: None, agent: None };
        s.runs.save(&log("aaaaaaaaaaaa", Some(&one.id))).unwrap();
        s.runs.save(&log("bbbbbbbbbbbb", Some(&two.id))).unwrap();
        s.runs.save(&log("cccccccccccc", None)).unwrap();
        assert_eq!(s.session_files("alice", &one.id), 2);
        assert!(!s.sessions.recall("alice", &one.id, "what is x", 3).is_empty());
        // someone else cannot delete it
        assert!(s.delete_session("bob", &one.id).is_err() && w1.join("a.txt").exists());
        let gone = s.delete_session("alice", &one.id).unwrap();
        assert_eq!(gone, SessionDeleted { runs: 1, files: 2 });
        assert!(!w1.exists(), "the folder and its files are gone");
        assert!(s.sessions.load("alice", &one.id).is_none());
        assert!(s.sessions.recall("alice", &one.id, "what is x", 3).is_empty(), "its memory is gone");
        let left: Vec<String> = s.runs.list("alice").into_iter().map(|r| r.id).collect();
        assert_eq!(left.len(), 2, "only that chat's run was removed: {left:?}");
        assert!(w2.join("keep.txt").exists() && s.sessions.load("alice", &two.id).is_some(), "other chats are untouched");
        assert!(s.delete_session("alice", &one.id).is_err(), "deleting twice is an error, not a crash");
    }

    type Reply = Box<dyn Fn(&str, &str) -> (u16, String) + Send + Sync>;
    struct Mock {
        calls: Mutex<Vec<(String, String)>>,
        reply: Mutex<Reply>,
    }
    impl Upstream for Mock {
        fn request(&self, m: &str, url: &str, _h: &[(String, String)], _b: &[u8]) -> Result<UpstreamResponse, String> {
            self.calls.lock().unwrap().push((m.into(), url.into()));
            let (status, body) = (self.reply.lock().unwrap())(m, url);
            Ok(UpstreamResponse { status, body: body.into_bytes() })
        }
    }

    fn svc(tag: &str, reply: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static) -> (Arc<Services>, Arc<Mock>, PathBuf) {
        let d = std::env::temp_dir().join(format!("iyakki-svc-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        let m = Arc::new(Mock { calls: Default::default(), reply: Mutex::new(Box::new(reply)) });
        (Services::open(&d, m.clone(), m.clone()).unwrap(), m, d)
    }

    fn none(_: &str, _: &str) -> (u16, String) {
        (500, String::new())
    }

    #[test]
    fn vault_lifecycle_across_logins() {
        let (s, _, _) = svc("life", none);
        // first admin creates the system vault and their own personal vault
        let r = s.on_login("root", "root-pass-1", true);
        assert_eq!((r.user_vault, r.system_vault), (VaultState::Created, VaultState::Created));
        s.system_set("openai-key", "sk-123").unwrap();
        s.user_set("root", "my-token", "personal").unwrap();
        // a plain user never touches the system vault, and has their own private one
        let r = s.on_login("amy", "amy-pass-123", false);
        assert_eq!((r.user_vault, r.system_vault), (VaultState::Created, VaultState::NotApplicable));
        s.user_set("amy", "note", "amys").unwrap();
        assert_eq!(s.user_names("amy").unwrap(), vec!["note"]);
        assert_eq!(s.user_names("root").unwrap(), vec!["my-token"], "vaults are separate");
        // while an admin has unlocked it, a second admin signing in gets their own slot
        let r = s.on_login("bob", "bob-pass-123", true);
        assert_eq!(r.system_vault, VaultState::Unlocked);
        assert!(s.system_slots().contains(&"user:bob".to_string()));
        // logout forgets personal vaults
        s.on_logout("amy", false);
        assert!(!s.user_vault_unlocked("amy") && s.user_names("amy").is_err());
        // back in, the data is still there
        s.on_login("amy", "amy-pass-123", false);
        assert_eq!(s.user_names("amy").unwrap(), vec!["note"]);
    }

    #[test]
    fn locked_system_vault_rules() {
        let (s, _, d) = svc("locked", none);
        s.on_login("root", "root-pass-1", true);
        s.system_set("k", "v").unwrap();
        s.lock_system();
        assert_eq!(s.system_names(), Err(VaultError::Locked));
        // a new admin has no slot while it is locked: stays locked, clear reason
        let r = s.on_login("eve", "eve-pass-123", true);
        assert!(matches!(r.system_vault, VaultState::Locked(ref m) if m.contains("no key slot")), "{:?}", r.system_vault);
        assert_eq!(s.system_names(), Err(VaultError::Locked));
        // the original admin unlocks it again with their password
        assert_eq!(s.on_login("root", "root-pass-1", true).system_vault, VaultState::Unlocked);
        // a wrong password does not unlock
        s.lock_system();
        let r = s.on_login("root", "WRONG-pass-1", true);
        assert!(matches!(r.system_vault, VaultState::Locked(_)));
        assert!(matches!(r.user_vault, VaultState::Locked(_)), "personal vault also refuses a wrong password");
        // a restart keeps the vault on disk, locked
        let m = Arc::new(Mock { calls: Default::default(), reply: Mutex::new(Box::new(none)) });
        let s2 = Services::open(&d, m.clone(), m).unwrap();
        assert_eq!(s2.system_vault_state(), (true, false));
        assert_eq!(s2.on_login("root", "root-pass-1", true).system_vault, VaultState::Unlocked);
        assert_eq!(s2.system_names().unwrap(), vec!["k"]);
    }

    #[test]
    fn lock_on_admin_logout_is_optional() {
        let (s, _, _) = svc("logout", none);
        s.on_login("root", "root-pass-1", true);
        s.on_logout("root", true);
        assert!(s.system_vault_state().1, "default: stays open so chat keeps working for everyone");
        s.settings.lock().unwrap().update(|x| x.vault.lock_on_admin_logout = true).unwrap();
        s.on_logout("root", true);
        assert!(!s.system_vault_state().1);
    }

    #[test]
    fn password_changes_and_resets() {
        let (s, _, _) = svc("pw", none);
        s.on_login("amy", "old-pass-123", false);
        s.user_set("amy", "k", "v").unwrap();
        s.on_logout("amy", false);
        s.on_password_change("amy", "old-pass-123", "new-pass-123");
        assert!(matches!(s.on_login("amy", "old-pass-123", false).user_vault, VaultState::Locked(_)));
        assert_eq!(s.on_login("amy", "new-pass-123", false).user_vault, VaultState::Unlocked);
        assert_eq!(s.user_names("amy").unwrap(), vec!["k"], "secrets survive a normal password change");
        // an admin reset can't recover them: the vault is replaced with a fresh empty one
        s.on_logout("amy", false);
        s.reset_user_vault("amy").unwrap();
        assert_eq!(s.on_login("amy", "admin-set-pw-1", false).user_vault, VaultState::Created);
        assert!(s.user_names("amy").unwrap().is_empty());
    }

    #[test]
    fn keyfile_unlocks_without_a_password() {
        let (s, _, d) = svc("keyfile", none);
        s.on_login("root", "root-pass-1", true);
        s.system_set("k", "v").unwrap();
        let kf = d.join("vault.key");
        s.enable_keyfile(kf.to_str().unwrap()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&kf).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(s.enable_keyfile(kf.to_str().unwrap()).is_err(), "never overwrites an existing key file");
        // simulate a reboot
        let m = Arc::new(Mock { calls: Default::default(), reply: Mutex::new(Box::new(none)) });
        let s2 = Services::open(&d, m.clone(), m.clone()).unwrap();
        assert_eq!(s2.system_vault_state(), (true, true), "unlocked at startup from the key file");
        assert_eq!(s2.system_names().unwrap(), vec!["k"]);
        // deleting the key file leaves it locked after the next start
        std::fs::remove_file(&kf).unwrap();
        let s3 = Services::open(&d, m.clone(), m).unwrap();
        assert_eq!(s3.system_vault_state(), (true, false));
    }

    #[test]
    fn chat_follows_settings_live_and_explains_problems() {
        let (s, m, _) = svc("chat", |meth, url| {
            if meth == "POST" && url.ends_with("/v1/chat/completions") {
                (200, r#"{"choices":[{"message":{"content":"hello from ollama"}}]}"#.into())
            } else {
                (404, String::new())
            }
        });
        let chat = s.chat_backend();
        // nothing configured yet: the message tells the user what to do
        assert!(chat.complete(&[Message::user("hi")]).unwrap_err().contains("Settings"));
        s.settings.lock().unwrap().upsert_provider(ProviderCfg::new("local", Kind::Ollama)).unwrap();
        s.settings.lock().unwrap().set_chat(Some(ModelChoice { provider: "local".into(), model: "qwen3:32b".into() })).unwrap();
        // no restart, no rebuild: the same backend object now answers
        assert_eq!(chat.complete(&[Message::user("hi")]).unwrap(), "hello from ollama");
        assert_eq!(m.calls.lock().unwrap().last().unwrap().1, "http://localhost:11434/v1/chat/completions");
        // a provider whose key lives in a locked vault fails closed with a clear reason
        let mut openai = ProviderCfg::new("openai", Kind::OpenAi);
        openai.key_ref = Some("vault:openai-key".into());
        s.settings.lock().unwrap().upsert_provider(openai).unwrap();
        s.settings.lock().unwrap().set_chat(Some(ModelChoice { provider: "openai".into(), model: "gpt-4o".into() })).unwrap();
        let before = m.calls.lock().unwrap().len();
        assert!(chat.complete(&[Message::user("hi")]).unwrap_err().contains("locked"));
        assert_eq!(m.calls.lock().unwrap().len(), before, "no request is made without the key");
    }

    #[test]
    fn embedding_choice_changes_the_embedder_id() {
        let (s, _, _) = svc("embid", none);
        let e = s.embedder();
        assert_eq!(e.id(), "hash-256");
        s.settings.lock().unwrap().upsert_provider(ProviderCfg::new("local", Kind::Ollama)).unwrap();
        s.settings.lock().unwrap().set_embedding(Some(ModelChoice { provider: "local".into(), model: "nomic-embed-text".into() })).unwrap();
        assert_eq!(e.id(), "remote:local:nomic-embed-text");
    }

    #[test]
    fn federated_search_merges_local_and_external_and_survives_failures() {
        let (s, _, _) = svc("fed", |meth, url| {
            if meth == "POST" && url == "http://rag.corp/search" {
                (200, r#"{"results":[{"text":"The handbook says deploy keys rotate quarterly.","source":"corp-wiki"},{"text":"Unrelated corporate text.","source":"wiki2"}]}"#.into())
            } else {
                (503, "down".into())
            }
        });
        s.kb.lock().unwrap().add_document("shared", "ops", "ops.md", "Deploy keys live in the vault under prod.", BTreeMap::new()).unwrap();
        {
            let mut st = s.settings.lock().unwrap();
            st.upsert_external(ExtEntry { enabled: true, backend: ExternalRag::Http { id: "corp".into(), url: "http://rag.corp/search".into(), key_ref: None } }).unwrap();
            st.upsert_external(ExtEntry { enabled: true, backend: ExternalRag::Qdrant { id: "broken".into(), url: "http://qdrant.down:6333".into(), collection: "docs".into(), key_ref: None } }).unwrap();
            st.upsert_external(ExtEntry { enabled: false, backend: ExternalRag::Http { id: "off".into(), url: "http://never".into(), key_ref: None } }).unwrap();
        }
        let (hits, warnings) = s.search(&["shared"], "deploy keys", 5);
        let sources: Vec<&str> = hits.iter().map(|h| h.source.as_str()).collect();
        assert!(sources.contains(&"ops.md") && sources.contains(&"corp-wiki"), "{sources:?}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("broken:"), "a failing backend is reported, not fatal");
        // switching external use off leaves only the local base
        s.settings.lock().unwrap().update(|x| x.rag.use_external = false).unwrap();
        let (hits, warnings) = s.search(&["shared"], "deploy keys", 5);
        assert!(hits.iter().all(|h| h.ns == "shared") && warnings.is_empty());
    }

    #[test]
    fn rank_fusion_merges_and_dedupes() {
        let h = |t: &str, ns: &str| Hit { ns: ns.into(), doc_id: t.into(), title: t.into(), source: t.into(), chunk_idx: 0, text: t.into(), score: 0.0, bm25: 0.0, vector: 0.0 };
        let fused = fuse(vec![vec![h("a", "x"), h("b", "x"), h("c", "x")], vec![h("c", "y"), h("a", "y")]], 3);
        assert_eq!(fused.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(), vec!["a", "c", "b"], "items found by both lists rise");
        assert!(fused[0].score > fused[2].score);
        assert!(fuse(vec![], 3).is_empty());
        assert_eq!(fuse(vec![vec![h("a", "x"), h("b", "x")]], 1).len(), 1);
    }

    #[test]
    fn changing_the_embedding_model_brings_the_documents_up_to_date_in_the_background() {
        let (s, _, _) = svc("autore", |meth, url| {
            if meth == "POST" && url.ends_with("/embeddings") {
                (200, r#"{"data":[{"embedding":[0.1,0.2,0.3]}]}"#.into())
            } else {
                (503, "down".into())
            }
        });
        s.kb.lock().unwrap().add_document("shared", "t", "s", "alpha beta gamma", BTreeMap::new()).unwrap();
        assert!(s.kb.lock().unwrap().stale_namespaces().is_empty());
        assert!(!s.reindex_stale_in_background(), "nothing to do while the model is unchanged");
        s.settings.lock().unwrap().upsert_provider(ProviderCfg::new("local", Kind::Ollama)).unwrap();
        s.settings.lock().unwrap().set_embedding(Some(ModelChoice { provider: "local".into(), model: "nomic-embed-text".into() })).unwrap();
        assert_eq!(s.kb.lock().unwrap().stale_namespaces(), vec!["shared".to_string()]);
        assert!(s.reindex_stale_in_background());
        for _ in 0..400 {
            if !s.is_reindexing() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!s.is_reindexing());
        let note = s.reindex_note().expect("a note");
        assert!(note.contains("remote:local:nomic-embed-text") || note.contains("could not"), "{note}");
    }

    #[test]
    fn a_local_ollama_that_comes_up_is_added_to_models_once_and_nothing_else_changes() {
        let (s, _, _) = svc("localollama", none);
        s.use_local_ollama("http://127.0.0.1:11434");
        s.use_local_ollama("http://127.0.0.1:11434/");
        let st = s.settings.lock().unwrap();
        let ps: Vec<_> = st.get().providers.iter().filter(|p| p.kind == Kind::Ollama).collect();
        assert_eq!(ps.len(), 1);
        assert_eq!((ps[0].id.as_str(), ps[0].base_url.as_deref()), ("ollama-local", Some("http://127.0.0.1:11434")));
        assert!(st.get().chat.is_none(), "the chat model is the person's choice");
    }

    #[test]
    fn an_ollama_already_running_on_this_machine_is_found_and_added_to_models() {
        use std::io::{Read, Write};
        let (s, _, _) = svc("detectollama", none);
        // nothing there: nothing added
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        s.detect_local_ollama_at(port);
        std::thread::sleep(std::time::Duration::from_millis(700));
        assert!(s.settings.lock().unwrap().get().providers.iter().all(|p| p.id != "ollama-local"));
        // something answering like Ollama: added
        let l = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        std::thread::spawn(move || {
            for _ in 0..3 {
                if let Ok((mut c, _)) = l.accept() {
                    let mut b = [0u8; 256];
                    let _ = c.set_read_timeout(Some(std::time::Duration::from_millis(300)));
                    let _ = c.read(&mut b);
                    let _ = c.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 20\r\n\r\n{\"version\":\"0.35.1\"}");
                }
            }
        });
        s.detect_local_ollama_at(port);
        for _ in 0..100 {
            if s.settings.lock().unwrap().get().providers.iter().any(|p| p.id == "ollama-local") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        let st = s.settings.lock().unwrap();
        let p = st.get().providers.iter().find(|p| p.id == "ollama-local").expect("added");
        assert_eq!(p.base_url.as_deref(), Some(format!("http://127.0.0.1:{port}").as_str()));
    }

    #[test]
    fn the_device_watch_reads_the_neighbour_table_in_the_background_and_reports_strangers_once() {
        let (s, _, _) = svc("devwatch", none);
        let arp = std::env::temp_dir().join(format!("iyakki-arp-{}", std::process::id()));
        let row = |ip: &str, mac: &str| format!("{ip} 0x1 0x2 {mac} * eth0\n");
        let head = "IP address HW type Flags HW address Mask Device\n";
        std::fs::write(&arp, format!("{head}{}", row("10.0.0.1", "aa:aa:aa:aa:aa:01"))).unwrap();
        s.start_device_watch_from(arp.clone(), std::time::Duration::from_millis(40));
        s.start_device_watch_from(arp.clone(), std::time::Duration::from_millis(40)); // a second call does nothing
        let wait = |f: &dyn Fn() -> bool| {
            for _ in 0..200 {
                if f() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            panic!("it never happened");
        };
        wait(&|| s.devwatch.list().len() == 1);
        assert!(s.devwatch.take_fresh().is_empty());
        std::fs::write(&arp, format!("{head}{}{}", row("10.0.0.1", "aa:aa:aa:aa:aa:01"), row("10.0.0.9", "bb:bb:bb:bb:bb:09"))).unwrap();
        wait(&|| s.devwatch.untrusted_count() == 1);
        let fresh = s.devwatch.take_fresh();
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].mac, "bb:bb:bb:bb:bb:09");
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(s.devwatch.take_fresh().is_empty(), "told once");
        let _ = std::fs::remove_file(&arp);
    }

    #[test]
    fn finished_and_failed_runs_leave_a_note_stopped_ones_do_not_and_warnings_are_not_repeated() {
        use crate::agent::{RunLimits, Tools};
        use crate::chat::{ChatBackend, Message, ModelTurn, ToolSpec};
        struct Say(Result<String, String>);
        impl ChatBackend for Say {
            fn complete(&self, _: &[Message]) -> Result<String, String> {
                Err("unused".into())
            }
            fn complete_tools(&self, _: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
                self.0.clone().map(|text| ModelTurn { text, calls: vec![] })
            }
        }
        struct NoTools;
        impl Tools for NoTools {
            fn defs(&self) -> Vec<crate::agent::ToolDef> {
                vec![]
            }
            fn call(&self, _: &str, _: &serde_json::Value) -> Result<String, String> {
                Ok(String::new())
            }
            fn describe(&self, n: &str, _: &serde_json::Value) -> String {
                n.into()
            }
        }
        let (s, _, _) = svc("inboxruns", |_, _| (200, String::new()));
        let run = |user: &str, r: Result<String, String>| {
            let mut run = crate::runs::RunSession::start(Some(s.runs.clone()), user, "write the summary of everything", Arc::new(Say(r)), Arc::new(NoTools), vec![], RunLimits::default());
            for _ in 0..500 {
                run.poll();
                if run.is_done() {
                    return run;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("run did not end");
        };
        let ok = run("amy", Ok("Here is the summary: three things happened.".into()));
        let notes = s.inbox.list("amy");
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].kind, notes[0].title.as_str(), notes[0].body.as_str()), (crate::inbox::Kind::Agent, "Finished: write the summary of everything", "Here is the summary: three things happened."));
        assert_eq!(notes[0].link, Some(crate::inbox::Link::new("runs", ok.id())));
        run("amy", Err("the model is down".into()));
        let notes = s.inbox.list("amy");
        assert!(notes.iter().any(|n| n.kind == crate::inbox::Kind::Error && n.title.starts_with("Failed:") && n.body.contains("the model is down")), "{notes:?}");
        assert!(s.inbox.list("bob").is_empty());
        // a run stopped by the person leaves nothing
        let st = crate::runs::RunSession::start(Some(s.runs.clone()), "cy", "x", Arc::new(Say(Ok("never".into()))), Arc::new(NoTools), vec![], RunLimits::default());
        st.cancel();
        drop(st);
        assert!(s.inbox.list("cy").iter().all(|n| n.kind != crate::inbox::Kind::Agent || n.title.starts_with("Finished")), "a stopped run has no failure note");
        // the helper the browser will use, and warnings that do not pile up
        s.notify("amy", crate::inbox::Kind::Download, "report.pdf downloaded", "1.2 MB", Some(crate::inbox::Link::new("browser", "dl1"))).unwrap();
        assert!(s.inbox.list("amy").iter().any(|n| n.kind == crate::inbox::Kind::Download && n.source == "browser"));
        for _ in 0..3 {
            s.notify_once("amy", crate::inbox::Kind::Warning, "The system vault is locked", "", None).unwrap();
        }
        assert_eq!(s.inbox.list("amy").iter().filter(|n| n.title == "The system vault is locked").count(), 1);
        assert!(s.notify("../x", crate::inbox::Kind::Info, "t", "", None).is_err());
    }

}
