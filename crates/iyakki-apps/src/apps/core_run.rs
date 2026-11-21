//! The chat run the core builds for a [`RunRequest`] (F-SPLIT-03, docs/development/UI-RPC-DESIGN.md section 6).
//!
//! The UI program has no agent, so it cannot build the tool chain: it sends the request as data and the core builds the chain here, with the
//! same wrappers ([`super::tool_chain`]), the same limits and the same tool routing the CHAT tile uses. [`CoreRunStarter`] is the real
//! `RunStarter`; it serves the account-level run (the CHAT tile, with the dashboard, flow, task list and cluster it shows given by name). A request that names an external agent
//! or Autopilot (it needs the project) is served too.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use iyakki_agent_api::{ChatRun, ExtRunSpec, RunRequest, RunStarter};
use iyakki_shared::agent::{RunLimits, ToolDef};
use iyakki_shared::chat::{Message, Role};
use iyakki_shared::context::ContextOptions;
use iyakki_shared::toolblock::COMPACT_PROMPT as AGENT_SYSTEM_PROMPT;

use super::AppCx;

/// What the context of a run turns on besides the account: the flow, dashboard and workspace groups.
#[derive(Clone, Copy, Debug, Default)]
pub struct AssistScope {
    pub flows: bool,
    pub dash: bool,
    pub project: bool,
}

/// The assistant harness of a run: the reminder, tool routing (Settings -> Models) and the thinking watchdog's limits (Settings).
pub fn assist_for(cx: &AppCx, scope: AssistScope) -> iyakki_shared::assistrun::Assist {
    let browser = match cx.session.policy.browser_agent {
        iyakki_sec::policy::BrowserAgent::Off => "off",
        iyakki_sec::policy::BrowserAgent::Interactive => "interactive",
        _ => "read-only",
    };
    let reminder = iyakki_shared::toolblock::ReminderCtx { agent_on: true, browser_policy: browser.into(), engine_installed: cx.agent.webengine_embedded_available(), screen_available: cx.screen().present(), loaded: None };
    let mut assist = iyakki_shared::assistrun::Assist::standard(reminder);
    {
        // tool routing (Settings -> Models): the groups this chat's context needs from the first request, or every tool
        use iyakki_shared::toolblock::Group;
        let machine_off = cx.settings().lock().unwrap_or_else(|e| e.into_inner()).get().no_tool_routing;
        assist.lazy_groups = cx.agent.toolrouting_lazy_groups(cx.data_dir(), &cx.session.user, machine_off);
        if scope.flows {
            assist.context_groups.insert(Group::Flows);
        }
        if scope.dash {
            assist.context_groups.insert(Group::Dash);
        }
        if scope.project {
            assist.context_groups.extend([Group::Workspace, Group::Cluster, Group::Build, Group::System, Group::Data]);
        }
    }
    {
        // the thinking watchdog's limits (Settings; 0 = the defaults of 60 s of pure thinking and 120 s of silence)
        let st = cx.settings().lock().unwrap_or_else(|e| e.into_inner());
        let provider = st.get().chat.as_ref().map(|c| c.provider.clone()).unwrap_or_default();
        let (t, q) = st.get().stall_limits(&provider);
        if t > 0 {
            assist.think_limit = std::time::Duration::from_secs(t as u64);
        }
        if q > 0 {
            assist.silence_limit = std::time::Duration::from_secs(q as u64);
        }
    }
    assist
}


/// What a workspace (AI IDE) chat adds to a run: the narrowed file scope, the project kit, the cluster tools and the system prompt block.
pub struct ProjectParts {
    pub policy: iyakki_sec::policy::Policy,
    pub workdir: std::path::PathBuf,
    pub system: String,
    pub kit: Option<Arc<dyn iyakki_agent_api::ProjectKit>>,
    pub platform: Option<Arc<dyn std::any::Any + Send + Sync>>,
    pub state_fn: Arc<dyn Fn() -> String + Send + Sync>,
}

/// The workspace part of a run, taking the project by name (`ProjectScope`) and the few flags of the chat instead of the chat's state:
/// the CHAT tile and the core's [`CoreRunStarter`] both call it.
pub fn project_parts(cx: &AppCx, p: &super::chat::ProjectScope, session: Option<&str>, autopilot: bool, has_tasks: bool, text: &str, opts: &mut ContextOptions) -> ProjectParts {
    let mut cluster_pair: Option<(iyakki_shared::devcluster::ClusterProfile, String)> = None;
    let mut platform: Option<Arc<dyn std::any::Any + Send + Sync>> = None;
    let root = p.root.display().to_string();
    let (user, base) = (cx.session.user.as_str(), &cx.session.policy);
    let mut policy = base.clone();
    policy.fs_read = if base.allows_read(user, &root) { vec![root.clone()] } else { vec![] };
    policy.fs_write = if base.allows_write(user, &root) { vec![root] } else { vec![] };
    let workdir = p.root.clone();
    // the workspace's connected cluster: the k8s_* and deploy_* tools, only for an account that may run commands
    let profile = p.cluster.as_ref().and_then(|c| c.slot.lock().ok().and_then(|g| g.resolution.as_ref().and_then(|r| r.reachable().cloned())));
    let may_run = cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full && (profile.as_ref().is_none_or(|pr| pr.local) || cx.session.policy.allows_host("*"));
    let mut kit_cluster = None;
    if let (Some(c), Some(pr), true) = (&p.cluster, profile, may_run) {
        cluster_pair = Some((pr.clone(), c.namespace.clone()));
        kit_cluster = Some(iyakki_agent_api::ProjectCluster { runner: c.runner.clone(), profile: pr, namespace: c.namespace.clone(), phrase: cx.agent.clusters_phrase_for(cx.data_dir(), &cx.session.user, &p.name) });
        // platform services (Gitea, a private store) are offered with the cluster tools; they always ask
        let env = if crate::TESTING { iyakki_shared::devcluster::Env::default() } else { iyakki_shared::devcluster::Env::real() };
        platform = Some(super::platform_services::platform_for(cx, c.runner.clone(), env).tools_any());
    }
    let kit: Option<Arc<dyn iyakki_agent_api::ProjectKit>> = {
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        cx.b4().project_kit(iyakki_agent_api::ProjectKitSpec { user: cx.session.user.clone(), name: p.name.clone(), root: p.root.clone(), features: p.features.clone(), audit: Arc::new(move |k, d| audit.record(&user, k, d)), session: session.map(str::to_string), cluster: kit_cluster })
    };
    let mut system = format!("{AGENT_SYSTEM_PROMPT}{}", cx.agent.helpers_b4().workspace_prompt(&p.name, &p.root, &p.features.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "FEATURES.md".into()), cluster_pair.as_ref().map(|(pr, ns)| (pr, ns.as_str()))));
    let state_fn = {
        let cfg = iyakki_shared::workspace::ProjectConfig::load(&p.root).unwrap_or_default();
        opts.pinned_skills = cx.agent.helpers_b4().project_pinned_skills(&p.root, &cfg.stack, text.trim());
        let f: Arc<dyn Fn() -> String + Send + Sync> = match kit.clone() {
            Some(k) => {
                let root = p.root.clone();
                Arc::new(move || {
                    let cfg = iyakki_shared::workspace::ProjectConfig::load(&root).unwrap_or_default();
                    k.state(&cfg.stack, &cfg.commands.test, &cfg.commands.run, &cfg.commands.build)
                })
            }
            None => Arc::new(String::new),
        };
        if !autopilot {
            system.push_str("\n\n");
            system.push_str(&f());
        }
        f
    };
    if cx.agent.b0().idebrowser().offered(&cx.session.policy) {
        system.push('\n');
        system.push_str(iyakki_shared::idebrowser::PROMPT_RULES);
    }
    if has_tasks && cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full {
        system.push_str(iyakki_shared::tasktools::PROMPT_RULE);
    }
    system.push_str(iyakki_shared::toolrouting::PROMPT_RULE);
    if iyakki_shared::gitworktools::offered(&cx.session.policy) {
        system.push_str(iyakki_shared::gitworktools::PROMPT_RULE);
    }
    system.push_str(&cx.services.msg_prompt_rules(&cx.session.user));
    if cx.session.policy.allows_app(iyakki_sec::policy::App::Rag) {
        system.push_str(iyakki_shared::ragtools::PROMPT_RULE);
    }
    system.push_str(iyakki_shared::updatestools::PROMPT_RULE);
    system.push_str(iyakki_shared::doctortools::PROMPT_RULE);
    system.push_str(iyakki_shared::manualtest::PROMPT_RULE);
    system.push_str(iyakki_shared::wizardtools::PROMPT_RULE);
    system.push_str(iyakki_shared::routertools::PROMPT_RULE);
    system.push_str(cx.services.fleet_prompt_rule());
    system.push_str(iyakki_shared::imagebuildtools::PROMPT_RULE);
    system.push_str(iyakki_shared::linuxpodtools::PROMPT_RULE);
    system.push_str(iyakki_shared::mcpadmintools::PROMPT_RULE);
    system.push_str(iyakki_shared::skillstools::PROMPT_RULE);
    system.push_str(iyakki_shared::linuxappimagetools::PROMPT_RULE);
    system.push_str(iyakki_shared::linuxsandboxtools::PROMPT_RULE);
    system.push_str(iyakki_shared::k8sexptools::PROMPT_RULE);
    system.push_str(iyakki_shared::sessionlogtools::PROMPT_RULE);
    system.push_str(iyakki_shared::lockscreentools::PROMPT_RULE);
    system.push_str(&super::chat_settings::prompt_rules(cx.session.policy.manage_settings));
    if cx.session.role == iyakki_sec::policy::Role::Admin {
        system.push_str(iyakki_shared::clientmedia::DEVICE_PROMPT_RULE);
        system.push_str(iyakki_shared::devmode::restarttools::PROMPT_RULE);
        system.push_str(iyakki_shared::devmode::restarttools::PAIRING_RULE);
    }
    system.push_str(iyakki_shared::enchometools::PROMPT_RULE);
    system.push_str(iyakki_shared::accessibilitytools::PROMPT_RULE);
    system.push_str(iyakki_shared::browsera11y::PROMPT_RULE);
    if cx.session.role != iyakki_sec::policy::Role::Guest {
        system.push_str(iyakki_shared::disksunlocktools::PROMPT_RULE);
        if cx.session.role == iyakki_sec::policy::Role::Admin {
            system.push_str(iyakki_shared::disksformattools::PROMPT_RULE);
        }
    }
    if cx.session.policy.manage_settings {
        system.push_str(iyakki_shared::componentstools::PROMPT_RULE);
        system.push_str(iyakki_shared::buildtools::PROMPT_RULE);
    }
    system.push_str(iyakki_shared::pdftools::PROMPT_RULE);
    if cx.session.policy.manage_settings {
        system.push_str(iyakki_shared::pdfnativetools::PROMPT_RULE);
    }
    system.push_str(iyakki_shared::tilestools::PROMPT_RULE);
    system.push_str(iyakki_shared::dictfixtools::PROMPT_RULE);
    system.push_str(iyakki_shared::voiceassisttools::PROMPT_RULE);
    system.push_str(cx.services.cloud_stream_prompt_rule());
    system.push_str(iyakki_shared::sharetools::PROMPT_RULE);
    system.push_str(iyakki_shared::browsershort::PROMPT_RULE);
    system.push_str(iyakki_shared::remotefilestools::PROMPT_RULE);
    if cx.session.policy.allows_app(iyakki_sec::policy::App::Cameras) {
        system.push_str(iyakki_shared::camerastools::PROMPT_RULE);
        system.push_str(iyakki_shared::camerastools::MOTION_RULE);
    }
    if cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full {
        system.push_str(iyakki_shared::sshsessiontools::PROMPT_RULE);
    }
    system.push_str(iyakki_shared::webgraphicstools::ACCEL_RULE);
    if cx.session.policy.manage_settings {
        system.push_str(iyakki_shared::sshtunneltools::PROMPT_RULE);
        system.push_str(iyakki_shared::domainstools::PROMPT_RULE);
        system.push_str(iyakki_shared::sftptools::PROMPT_RULE);
        system.push_str(iyakki_shared::webgraphicstools::PROMPT_RULE);
        system.push_str(iyakki_shared::objectstoragetools::PROMPT_RULE);
        system.push_str(iyakki_shared::ollamatools::PROMPT_RULE);
        system.push_str(iyakki_shared::modelopttools::PROMPT_RULE);
        system.push_str(iyakki_shared::quickuninstalltools::PROMPT_RULE);
        system.push_str(iyakki_shared::lemonadecomputetools::PROMPT_RULE);
        system.push_str(iyakki_shared::thiscomputertools::PROMPT_RULE);
        system.push_str(iyakki_shared::sharedvaulttools::PROMPT_RULE);
        system.push_str(iyakki_shared::vpntools::PROMPT_RULE);
        system.push_str(iyakki_shared::bluetoothtools::PROMPT_RULE);
        system.push_str(iyakki_shared::btaudiotools::PROMPT_RULE);
        system.push_str(iyakki_shared::bluetoothpair::PROMPT_RULE);
    }
    if kit.as_ref().and_then(|k| k.cluster_line()).is_none() {
        system.push_str("\nNo cluster is connected to this workspace. Never start, install or stop a cluster (k3s server, minikube, kind, kubeadm): it is refused. If the person wants to deploy, tell them to use the K8S panel's Change cluster, or to set a cluster up themselves.");
    }
    ProjectParts { policy, workdir, system, kit, platform, state_fn }
}

/// The workspace's tool wrappers around the account's tools: the IDE Browser tab tools, then the project kit (board, deploy, cluster, platform).
pub fn wrap_project_tools(cx: &AppCx, tools: Arc<dyn iyakki_shared::agent::Tools>, ide_policy: iyakki_sec::policy::Policy, p: &super::chat::ProjectScope, kit: Option<&Arc<dyn iyakki_agent_api::ProjectKit>>, platform: Option<Arc<dyn std::any::Any + Send + Sync>>) -> Arc<dyn iyakki_shared::agent::Tools> {
    // the IDE's Browser tab tools sit under the board tools, so the harness's http check of a deploy row can use them
    let base: Arc<dyn iyakki_shared::agent::Tools> = if cx.agent.b0().idebrowser().offered(&ide_policy) {
        cx.b0().ide_browser_tools(tools, ide_policy, &cx.session.user, &p.name, &p.root)
    } else {
        tools
    };
    match kit {
        Some(k) => k.wrap(base, platform),
        None => base,
    }
}


/// A state of the UI a run reads, handed to the core under a name (UI-RPC-DESIGN section 6: "the core is given those by name").
#[derive(Clone)]
pub enum Scope {
    Dash(super::chat::DashLink),
    Flows(super::chat::FlowsLink),
    Tasks(iyakki_agent_api::pending::TaskList),
    Cluster(super::chat::ClusterLink),
}

impl Scope {
    fn kind(&self) -> &'static str {
        match self {
            Scope::Dash(_) => "dash",
            Scope::Flows(_) => "flows",
            Scope::Tasks(_) => "tasks",
            Scope::Cluster(_) => "cluster",
        }
    }
}

/// The attached states, by name and owner. A name is only resolved for the user it was given for.
#[derive(Default)]
pub struct ScopeRegistry {
    next: std::sync::atomic::AtomicU64,
    held: Mutex<HashMap<String, (String, Scope)>>,
}

impl ScopeRegistry {
    /// Keep a state for `user` and answer its name.
    pub fn attach(&self, user: &str, scope: Scope) -> String {
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("{}:{n}", scope.kind());
        self.held.lock().unwrap_or_else(PoisonError::into_inner).insert(name.clone(), (user.to_string(), scope));
        name
    }

    /// Let a state go (an unknown name is ignored).
    pub fn detach(&self, name: &str) {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).remove(name);
    }

    fn get(&self, user: &str, name: &str) -> Result<Scope, String> {
        match self.held.lock().unwrap_or_else(PoisonError::into_inner).get(name) {
            Some((owner, s)) if owner == user => Ok(s.clone()),
            _ => Err(format!("there is no attached state {name} for {user}")),
        }
    }
}

/// The registry of this process: the shell attaches what its chats show, the core's [`CoreRunStarter`] resolves the names of a request.
pub fn scopes() -> &'static ScopeRegistry {
    static R: std::sync::OnceLock<ScopeRegistry> = std::sync::OnceLock::new();
    R.get_or_init(ScopeRegistry::default)
}

/// Why Autopilot cannot drive the external agent `id` (`None`: it can). Autopilot needs an agent that can use IYAKKI's tools.
pub(in crate::apps) fn autopilot_refusal(cx: &AppCx, id: &str) -> Option<String> {
        let a = cx.agents_cfg().agent(id)?;
        (a.mode() != iyakki_extagentx::Mode::Bridged).then(|| if id == iyakki_extagentx::COPILOT {
            "Autopilot needs an agent that works through IYAKKI's tools and approvals. GitHub Copilot normally does, but it is set to advisor only (read-only) in Settings → Agents: switch Advisor only off there, or pick a built-in model or Claude Code (Ctrl+P), or turn Autopilot off to chat with it step by step.".to_string()
        } else {
            format!("Autopilot needs an agent that can use IYAKKI's tools; Claude Code and GitHub Copilot can, {} runs read-only here (it can only advise). Pick a built-in model, Claude Code or GitHub Copilot (Ctrl+P) for Autopilot, or turn Autopilot off to chat with {} step by step.", a.label(), a.label())
        })
}

/// Start a run with the external agent `id`: the same tools, approvals and policy as the built-in assistant, through the bridge.
#[allow(clippy::too_many_arguments)]
pub(in crate::apps) fn run_external(cx: &AppCx, id: &str, sid: Option<String>, model: String, text: &str, tools: Arc<dyn iyakki_shared::agent::Tools>, limits: RunLimits, prepare: Option<iyakki_shared::assistrun::Prepare>, workspace: Option<std::path::PathBuf>) -> Result<Box<dyn ChatRun>, String> {
    let req = make_request(cx, id, text, tools, limits, prepare, workspace, sid, model)?;
    let h = cx.extagent_facet().start(req);
    cx.b4().chat_run_from_handle(&cx.session.user, text, h).ok_or_else(|| "the agent core serves no chat runs".to_string())
}

/// The Autopilot steps carried out by an external agent: each step's message is its prompt, resuming its own session, so it keeps its
/// context; the harness keeps the board, the checks, the caps and the summary.
pub(in crate::apps) fn ext_executor(cx: &AppCx, id: &str, sid: Option<String>, model: String, tools: Arc<dyn iyakki_shared::agent::Tools>, workspace: Option<std::path::PathBuf>) -> iyakki_shared::autopilot::Executor {
    let (cx, id) = (cx.clone(), id.to_string());
    Arc::new(move |call: iyakki_shared::autopilot::StepCall| {
        match make_request(&cx, &id, &call.prompt, tools.clone(), call.limits, call.prepare, workspace.clone(), sid.clone(), model.clone()) {
            Ok(req) => cx.extagent_facet().start(req),
            Err(e) => {
                let (tx, rx) = std::sync::mpsc::channel();
                let (dtx, _drx) = std::sync::mpsc::channel();
                let _ = tx.send(iyakki_shared::agent::RunEvent::Failed { error: e });
                iyakki_shared::agent::RunHandle::from_parts(rx, dtx)
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
pub(in crate::apps) fn make_request(cx: &AppCx, id: &str, text: &str, tools: Arc<dyn iyakki_shared::agent::Tools>, limits: RunLimits, prepare: Option<iyakki_shared::assistrun::Prepare>, workspace: Option<std::path::PathBuf>, sid: Option<String>, model: String) -> Result<ExtRunSpec, String> {
    let cfg = cx.agents_cfg();
    cfg.agent(id).ok_or("That agent is no longer configured. Pick a model again (Ctrl+P).")?;
    let workspace = workspace.ok_or("This chat has no work folder to run an agent in.")?;
    let (allow_auto, key_ref) = cfg.cfg_of(id).unwrap_or_default();
    // a login kept in the person's vault (Copilot): its token goes to this one run as the variable the program reads
    let key = if key_ref.is_empty() { cx.services.agent_vault_token(&cx.session.user, id).map(|(_, t)| t) } else { Some(cx.secrets().resolve(&key_ref, None).map_err(|e| format!("The API key `{key_ref}` could not be read from the vault: {e}. Open Settings → Agents to pick another."))?) };
    let resume = sid.as_ref().and_then(|s| cx.ai().ext_session(s, id));
    let (ai, user) = (cx.ai(), cx.session.user.clone());
    let (tools2, audit, user2) = (tools.clone(), cx.audit.clone(), cx.session.user.clone());
    let (ai2, sid2, aid2) = (ai.clone(), sid.clone(), id.to_string());
    Ok(ExtRunSpec {
        agent: id.to_string(),
        workspace,
        prompt: text.to_string(),
        system: Arc::new(move || match &prepare {
            Some(p) => p(&tools2.defs()).iter().filter(|m| m.role == Role::System).map(|m| m.content.clone()).collect::<Vec<_>>().join("\n\n"),
            None => String::new(),
        }),
        model,
        resume,
        tools,
        limits,
        allow_auto,
        key,
        audit: Arc::new(move |k, d| audit.record(&user2, k, d)),
        exe: std::env::current_exe().unwrap_or_default(),
        on_session: Arc::new(move |s| {
            if let Some(sid) = &sid2 {
                ai2.ext_session_set(sid, &aid2, s);
            }
        }),
        on_usage: Arc::new(move |l, m, i, o| ai.usage_record(&user, l, m, i, o)),
        no_tools: false,
    })
}


/// The AI IDE's Autopilot: a series of runs toward the goal (see `iyakki_agent::autopilot`). `resume` continues a paused series; a message typed while one
/// is paused is an interjection to re-plan for, not a new goal.
#[allow(clippy::too_many_arguments)]
pub(in crate::apps) fn autopilot_run(cx: &AppCx, p: &super::chat::ProjectScope, session: Option<String>, text: &str, resume: bool, state: Arc<dyn Fn() -> String + Send + Sync>, executor: Option<iyakki_shared::autopilot::Executor>, tools: Arc<dyn iyakki_shared::agent::Tools>, limits: RunLimits, assist: iyakki_shared::assistrun::Assist) -> Result<Box<dyn ChatRun>, String> {
    let (mut resume, mut goal, mut interjection) = (resume, text.trim().to_string(), None);
    if !resume {
        if let Some(s) = cx.agent.helpers_b4().autopilot_saved(&p.root.join(".iyakki/autopilot.json")).filter(|s| s.status == "paused") {
            resume = true;
            goal = s.goal.clone();
            interjection = Some(text.trim().to_string());
        }
    }
    let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
    let (svc, user2) = (cx.services.clone(), cx.session.user.clone());
    let history = (!cx.settings().lock().unwrap_or_else(PoisonError::into_inner).get().no_workspace_history).then(|| (p.name.clone(), session.clone()));
    let start = iyakki_agent_api::ApStart {
        root: p.root.clone(),
        features: p.features.clone(),
        goal,
        resume,
        interjection,
        session: session.unwrap_or_default(),
        state: Some(state),
        audit: Some(Arc::new(move |k, d| audit.record(&user, if k.starts_with("autopilot") || k == "board.auto" { k } else { "autopilot.other" }, d))),
        notify: Some(Arc::new(move |title, body| {
            let _ = svc.inbox().add(&user2, iyakki_shared::inbox::Kind::Agent, title, body, "autopilot", Some(iyakki_shared::inbox::Link::new("inbox", "")), iyakki_clock::now_unix());
        })),
        // an external coding agent carries out the steps (the harness keeps the board, the checks, the caps, the summary)
        executor,
        history,
    };
    let h = cx.b4().autopilot_start(&cx.session.user, cx.chat.clone(), tools, vec![Message::user(text.trim())], limits, Box::new(assist), start);
    cx.b4().chat_run_from_handle(&cx.session.user, text.trim(), h).ok_or_else(|| "the agent core serves no chat runs".to_string())
}

/// The most tokens of a request the condenser lets through (the same cap as the CHAT tile).
const REQUEST_CAP_TOKENS: u64 = 6000;

/// Builds the context of an account for a user name (the core gives it its session, the machine and the services).
pub type CxFor = Arc<dyn Fn(&str) -> Option<AppCx> + Send + Sync>;

/// A run the core builds and drives (its tool chain, its approvals' rules, its budget are the core's), seen by the chat as the same [`ChatRun`] it reads
/// events from (F-SPLIT-03: the UI program has no agent). Every call is a call on the core's [`iyakki_agent_api::ChatRuns`] over the socket: `poll` takes the new
/// events (the chat asks on every heartbeat), `decide` is the person's answer to an approval card, `cancel` and `pause` are the Stop and Pause buttons. A run that
/// is dropped while it still goes is stopped, and the core forgets it either way, so nothing keeps running unseen after the chat is closed.
pub struct RemoteChatRun {
    runs: Arc<dyn iyakki_agent_api::ChatRuns>,
    user: String,
    id: String,
}

/// Start the run the request describes in the core; the core's sentence when it refuses (the agent is off for the account, too many runs).
pub fn remote_run(runs: Arc<dyn iyakki_agent_api::ChatRuns>, req: RunRequest) -> Result<Box<dyn ChatRun>, String> {
    let user = req.user.clone();
    let id = runs.start(req)?;
    Ok(Box::new(RemoteChatRun { runs, user, id }))
}

impl ChatRun for RemoteChatRun {
    fn id(&self) -> String {
        self.id.clone()
    }
    fn is_done(&self) -> bool {
        self.runs.is_done(&self.user, &self.id)
    }
    fn poll(&mut self) -> Vec<iyakki_shared::agent::RunEvent> {
        self.runs.poll(&self.user, &self.id)
    }
    fn decide(&mut self, call_id: &str, v: iyakki_shared::agent::Verdict) {
        let _ = self.runs.decide(&self.user, &self.id, call_id, v);
    }
    fn cancel(&self) {
        let _ = self.runs.cancel(&self.user, &self.id);
    }
    fn pause(&self) {
        let _ = self.runs.pause(&self.user, &self.id);
    }
    // the session of a run is the request's (`RunRequest::session`): the core already logs it there
    fn in_session(self: Box<Self>, _session: &str) -> Box<dyn ChatRun> {
        self
    }
}

impl Drop for RemoteChatRun {
    fn drop(&mut self) {
        if !self.runs.is_done(&self.user, &self.id) {
            let _ = self.runs.cancel(&self.user, &self.id);
        }
        self.runs.forget(&self.user, &self.id);
    }
}

/// The real [`RunStarter`] of the core.
pub struct CoreRunStarter {
    cx_for: CxFor,
    /// The session approvals ("allow for this session") of each chat, kept for the chat's life: `(user, chat session)`.
    grants: Mutex<HashMap<(String, String), Arc<iyakki_shared::grants::Grants>>>,
}

impl CoreRunStarter {
    pub fn new(cx_for: CxFor) -> CoreRunStarter {
        CoreRunStarter { cx_for, grants: Mutex::new(HashMap::new()) }
    }

    fn grants_for(&self, user: &str, session: &str) -> Option<Arc<iyakki_shared::grants::Grants>> {
        if session.is_empty() {
            return None;
        }
        let mut g = self.grants.lock().unwrap_or_else(PoisonError::into_inner);
        Some(g.entry((user.to_string(), session.to_string())).or_insert_with(iyakki_shared::grants::Grants::new).clone())
    }
}

impl RunStarter for CoreRunStarter {
    fn start(&self, req: &RunRequest) -> Result<Box<dyn ChatRun>, String> {
        let cx = (self.cx_for)(&req.user).ok_or_else(|| format!("there is no account {}", req.user))?;
        if !cx.session.policy.agent {
            cx.audit.record(&cx.session.user, "agent.denied", "start");
            return Err("The agent is not enabled for your account.".into());
        }
        if let (Some(id), true) = (&req.ext, req.autopilot) {
            if let Some(why) = autopilot_refusal(&cx, id) {
                return Err(why);
            }
        }
        let text = req.prompt.trim().to_string();
        let reg = scopes();
        let want = |n: &Option<String>| n.as_deref().map(|n| reg.get(&req.user, n)).transpose();
        let dash = match want(&req.dash)? {
            Some(Scope::Dash(d)) => Some(d),
            None => None,
            Some(_) => return Err("the attached state named for the dashboard is not a dashboard".into()),
        };
        let flows = match want(&req.flows)? {
            Some(Scope::Flows(f)) => Some(f),
            None => None,
            Some(_) => return Err("the attached state named for the flow is not a flow".into()),
        };
        let tasks = match want(&req.tasks)? {
            Some(Scope::Tasks(t)) => Some(t),
            None => None,
            Some(_) => return Err("the attached state named for the task list is not a task list".into()),
        };
        let cluster = match want(&req.cluster)? {
            Some(Scope::Cluster(c)) => Some(c),
            None => None,
            Some(_) => return Err("the attached state named for the cluster is not a cluster".into()),
        };
        // every chat has a work folder of its own; the account may read and (if it may write at all) write there
        let mut policy = cx.session.policy.clone();
        let mut workdir = (!req.session.is_empty()).then(|| cx.session_workdir(&cx.session.user, &req.session).ok()).flatten();
        if let Some(d) = &workdir {
            let p = d.display().to_string();
            if !policy.fs_write.is_empty() {
                policy.fs_write.push(p.clone());
            }
            if !policy.fs_read.is_empty() || !policy.fs_write.is_empty() {
                policy.fs_read.push(p);
            }
        }
        let mut system = match &workdir {
            Some(d) => format!("{AGENT_SYSTEM_PROMPT}\nYour working folder for this chat is {}. Relative paths and new files go there; each chat has its own folder.", d.display()),
            None => AGENT_SYSTEM_PROMPT.to_string(),
        };
        // the assistant preset the person chose in Settings (text from a bundle: style and focus only)
        system.push_str(&iyakki_shared::bundle_members::preset_note(cx.data_dir(), &cx.session.user));
        // recalling memory and condensing the history happen on the run's own thread, so starting a run never waits
        let mut opts = ContextOptions { use_docs: false, ..Default::default() };
        // a workspace chat: the file scope narrows to the folder and the project kit and its prompt block join (the cluster and the task list are given by name)
        let project = req.project.as_ref().map(|(name, root)| super::chat::ProjectScope { name: name.clone(), root: root.into(), key: format!("ide:{name}"), features: std::path::Path::new(root).join("FEATURES.md"), cluster: cluster.clone() });
        let mut parts = None;
        let mut state_fn = None;
        if let Some(p) = &project {
            let pp = project_parts(&cx, p, (!req.session.is_empty()).then_some(req.session.as_str()), false, tasks.is_some(), &text, &mut opts);
            policy = pp.policy.clone();
            workdir = Some(pp.workdir.clone());
            state_fn = Some(pp.state_fn.clone());
            system = pp.system.clone();
            parts = Some(pp);
        }
        // detached jobs: anything slow goes through job_start (it outlives this chat and shows in the Jobs strip)
        if cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full {
            system.push_str(iyakki_shared::jobtools::PROMPT_RULE);
        }
        system.push_str(iyakki_shared::pendingtools::PROMPT_RULE);
        // the DASH tile: the rules for working on the open dashboard and a compact picture of it
        if let Some(d) = &dash {
            system.push_str(iyakki_dash::tools::PROMPT_RULES);
            system.push_str("\n\n");
            system.push_str(&iyakki_dash::tools::dash_context(&d.env, &d.state));
        }
        // the FLOWS tile: the rules for working on workflows and a compact, redacted picture of what is open
        if let Some(f) = &flows {
            system.push_str(f.env.prompt_rules());
            system.push_str("\n\n");
            system.push_str(&f.env.context(&f.open));
        }
        if let Some(line) = cx.agent.helpers_b4().data_tab_context(&cx.session.user) {
            system.push_str("\n\n");
            system.push_str(&line);
        }
        let (user_key, sid, before) = (cx.session.user.clone(), (!req.session.is_empty()).then(|| req.session.clone()), req.history.clone());
        let (svc, ask) = (cx.services.clone(), text.clone());
        let window = opts.window;
        let prepare: iyakki_shared::assistrun::Prepare = Arc::new(move |defs: &[ToolDef]| {
            let (history, _info) = svc.chat_context(&user_key, sid.as_deref(), &system, &before, &ask, &opts);
            let names: Vec<&str> = defs.iter().map(|d| d.spec.name.as_str()).collect();
            // what no longer fits the window is reduced to a few lines
            let cut = &before[..before.len().saturating_sub(window)];
            let older = iyakki_shared::assistant_guard::extractive_summary(cut, "");
            iyakki_shared::assistant_guard::condense(history, &names, REQUEST_CAP_TOKENS, &older).history
        });
        let policy_for_ide = policy.clone();
        let tools: Arc<dyn iyakki_shared::agent::Tools> = cx.services.user_tools(&cx.session.user, policy, cx.host.clone(), cx.mcp_servers.clone(), Some(&text), workdir.as_deref());
        let tools = match (&project, parts) {
            (Some(p), Some(pp)) => wrap_project_tools(&cx, tools, policy_for_ide, p, pp.kit.as_ref(), pp.platform),
            _ => tools,
        };
        let tools: Arc<dyn iyakki_shared::agent::Tools> = match &dash {
            Some(d) => Arc::new(iyakki_dash::tools::DashTools::new(tools, d.env.clone(), d.state.clone())),
            None => tools,
        };
        let tools = match &flows {
            Some(f) => f.env.wrap_tools(tools, f.open.clone()),
            None => tools,
        };
        let tools = match (&tasks, &project) {
            (Some(m), Some(_)) if cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full => cx.tasks_facet().wrap_task_tools(tools, m, req.ext.as_deref().unwrap_or("builtin"), ""),
            _ => tools,
        };
        let scope = super::tool_chain::ChainScope { project: project.as_ref().map(|p| (p.name.clone(), p.root.clone())), ext: req.ext.clone() };
        let tools = super::tool_chain::account_chain(&cx, tools, &scope);
        let tools = super::tool_chain::session_state_tools(&cx, tools, workdir.clone(), project.as_ref().map(|p| (p.name.as_str(), p.root.as_path())), tasks);
        cx.audit.record(&cx.session.user, "agent.start", &format!("{} chars", text.len()));
        let mut assist = assist_for(&cx, AssistScope { flows: flows.is_some(), dash: dash.is_some(), project: project.is_some() });
        assist.prepare = Some(prepare);
        let price = cx.settings().lock().unwrap_or_else(PoisonError::into_inner).get().token_price_cents_per_mtok;
        let limits = super::chat::browser_limits(&cx.session.policy, RunLimits { auto: req.auto_approve, grants: self.grants_for(&cx.session.user, &req.session), ..RunLimits::for_budget(&cx.session.policy.budget, price) });
        let sid = (!req.session.is_empty()).then(|| req.session.clone());
        let ext_model = |id: &str| req.ext_model.clone().unwrap_or_else(|| cx.agents_cfg().agent(id).map(|a| a.cfg_model()).unwrap_or_default());
        let run = match (&req.ext, &project) {
            // an external coding agent answers: it gets the same tools, approvals and policy through the bridge
            (Some(id), _) if !req.autopilot => run_external(&cx, id, sid.clone(), ext_model(id), &text, tools, limits, assist.prepare.clone(), workdir)?,
            (_, Some(p)) if req.autopilot => {
                let executor = req.ext.as_ref().map(|id| ext_executor(&cx, id, sid.clone(), ext_model(id), tools.clone(), workdir.clone()));
                autopilot_run(&cx, p, sid, &text, req.resume, state_fn.ok_or("a workspace run has its project state")?, executor, tools, limits, assist)?
            }
            _ => cx.b4().chat_run_start(&cx.session.user, &text, cx.chat.clone(), tools, vec![Message::user(&text)], limits, Box::new(assist)).ok_or("the agent core does not serve chat runs")?,
        };
        Ok(match req.session.as_str() {
            "" => run,
            s => run.in_session(s),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted `ChatRuns`: what a core says to the program over the socket, with every call the program made noted, and no clock.
    #[derive(Default)]
    struct Script {
        state: Mutex<ScriptState>,
    }
    #[derive(Default)]
    struct ScriptState {
        events: Vec<iyakki_shared::agent::RunEvent>,
        said: Vec<String>,
        started: Option<RunRequest>,
        refuse: bool,
        done: bool,
    }
    impl Script {
        fn said(&self) -> Vec<String> {
            self.state.lock().unwrap().said.clone()
        }
    }
    impl iyakki_agent_api::ChatRuns for Script {
        fn start(&self, req: RunRequest) -> Result<String, String> {
            let mut st = self.state.lock().unwrap();
            if st.refuse {
                return Err("The agent is not enabled for your account.".into());
            }
            st.started = Some(req);
            Ok("run-1".into())
        }
        fn poll(&self, user: &str, run: &str) -> Vec<iyakki_shared::agent::RunEvent> {
            let mut st = self.state.lock().unwrap();
            st.said.push(format!("poll {user} {run}"));
            std::mem::take(&mut st.events)
        }
        fn is_done(&self, _user: &str, _run: &str) -> bool {
            let st = self.state.lock().unwrap();
            st.done || (st.events.is_empty() && st.said.iter().any(|s| s == "cancel"))
        }
        fn decide(&self, _user: &str, _run: &str, call: &str, v: iyakki_shared::agent::Verdict) -> Result<(), String> {
            self.state.lock().unwrap().said.push(format!("decide {call} {v:?}"));
            Ok(())
        }
        fn cancel(&self, _user: &str, _run: &str) -> Result<(), String> {
            self.state.lock().unwrap().said.push("cancel".into());
            Ok(())
        }
        fn pause(&self, _user: &str, _run: &str) -> Result<(), String> {
            self.state.lock().unwrap().said.push("pause".into());
            Ok(())
        }
        fn forget(&self, _user: &str, _run: &str) {
            self.state.lock().unwrap().said.push("forget".into());
        }
    }

    #[test]
    fn a_run_of_the_core_is_the_run_the_chat_reads_events_from_and_the_core_forgets_it_when_the_chat_lets_it_go() {
        use iyakki_shared::agent::RunEvent;
        let runs = Arc::new(Script::default());
        let evs = vec![RunEvent::Model { text: "hi".into(), calls: vec![] }, RunEvent::Final { text: "hello".into() }];
        {
            let mut st = runs.state.lock().unwrap();
            st.events = evs.clone();
            st.done = true;
        }
        let mut r = req("ann");
        r.session = "s1".into();
        r.history = vec![Message::user("earlier")];
        let mut run = remote_run(runs.clone(), r.clone()).unwrap();
        assert_eq!(run.id(), "run-1");
        assert_eq!(runs.state.lock().unwrap().started.as_ref(), Some(&r), "the core got the request as it was");
        assert_eq!(run.poll(), evs, "the events of the core, in order");
        assert!(run.poll().is_empty(), "and only once");
        assert!(run.is_done());
        let run = run.in_session("s1");
        drop(run);
        let said = runs.said();
        assert_eq!(said.last().map(String::as_str), Some("forget"), "{said:?}");
        assert!(!said.contains(&"cancel".to_string()), "a run that is over is not cancelled: {said:?}");
    }

    #[test]
    fn the_persons_answer_to_an_approval_card_and_the_stop_and_pause_buttons_reach_the_core() {
        use iyakki_shared::agent::Verdict;
        let runs = Arc::new(Script::default());
        let mut run = remote_run(runs.clone(), req("ann")).unwrap();
        run.decide("c1", Verdict::Approve);
        run.decide("c2", Verdict::Deny);
        run.pause();
        run.cancel();
        let said = runs.said();
        assert_eq!(said.iter().filter(|s| s.starts_with("decide")).count(), 2, "{said:?}");
        assert!(said.iter().any(|s| s.starts_with("decide c1")) && said.iter().any(|s| s.starts_with("decide c2")), "{said:?}");
        assert!(said.contains(&"pause".to_string()) && said.contains(&"cancel".to_string()), "{said:?}");
    }

    #[test]
    fn a_chat_that_drops_a_run_that_still_goes_stops_it_first_and_a_refused_start_is_the_cores_sentence() {
        let runs = Arc::new(Script::default());
        let run = remote_run(runs.clone(), req("ann")).unwrap();
        drop(run);
        let said = runs.said();
        let (cancel, forget) = (said.iter().position(|s| s == "cancel"), said.iter().position(|s| s == "forget"));
        assert!(cancel.is_some() && forget.is_some() && cancel < forget, "stopped, then forgotten: {said:?}");
        let runs = Arc::new(Script::default());
        runs.state.lock().unwrap().refuse = true;
        let err = remote_run(runs, req("ann")).err().unwrap();
        assert_eq!(err, "The agent is not enabled for your account.");
    }

    fn starter(tag: &'static str) -> CoreRunStarter {
        CoreRunStarter::new(Arc::new(move |user| (user == "root").then(|| super::super::tests::cx_with_admin_store(tag))))
    }

    fn req(user: &str) -> RunRequest {
        RunRequest { user: user.into(), prompt: "hello".into(), ..Default::default() }
    }

    #[test]
    fn an_unknown_account_and_an_agent_that_is_not_configured_are_refused_by_name() {
        let s = starter("core-run-refuse");
        assert!(s.start(&req("nobody")).err().unwrap().contains("no account"));
        let mut r = req("root");
        r.ext = Some("claude".into());
        assert!(s.start(&r).err().unwrap().contains("no longer configured"));
    }

    #[test]
    fn an_account_run_starts_in_the_core_with_the_chats_grants_kept_per_session() {
        let s = starter("core-run-start");
        let mut r = req("root");
        r.session = "s1".into();
        let run = s.start(&r).expect("the core builds the chain and starts the run");
        assert!(!run.id().is_empty());
        run.cancel();
        let (a, b) = (s.grants_for("root", "s1").unwrap(), s.grants_for("root", "s1").unwrap());
        assert!(Arc::ptr_eq(&a, &b), "one chat session keeps one set of grants");
        assert!(s.grants_for("root", "").is_none(), "a run without a chat session keeps none");
    }

    #[test]
    fn a_workspace_run_starts_in_the_core_with_the_project_given_by_name() {
        let s = starter("core-run-project");
        let root = std::env::temp_dir().join(format!("iyakki-core-run-project-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut r = req("root");
        r.session = "s2".into();
        r.project = Some(("demo".into(), root.display().to_string()));
        let run = s.start(&r).expect("the core builds the workspace chain from the name and the folder");
        run.cancel();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_attached_state_is_resolved_only_by_its_name_for_its_owner_and_only_as_its_kind() {
        let reg = ScopeRegistry::default();
        let cluster = Scope::Cluster(super::super::chat::ClusterLink { slot: Default::default(), namespace: "dev".into(), runner: Arc::new(iyakki_shared::wsexec::FakeRunner::new(&[])) });
        let name = reg.attach("root", cluster);
        assert!(name.starts_with("cluster:"));
        assert!(matches!(reg.get("root", &name), Ok(Scope::Cluster(_))));
        assert!(reg.get("ann", &name).is_err(), "another account cannot use the name");
        reg.detach(&name);
        assert!(reg.get("root", &name).is_err(), "a detached name is gone");
        // a name of the wrong kind in a request is refused by the starter
        let s = starter("core-run-scope-kind");
        let n2 = scopes().attach("root", Scope::Cluster(super::super::chat::ClusterLink { slot: Default::default(), namespace: "dev".into(), runner: Arc::new(iyakki_shared::wsexec::FakeRunner::new(&[])) }));
        let mut r = req("root");
        r.dash = Some(n2.clone());
        assert!(s.start(&r).err().unwrap().contains("not a dashboard"));
        r.dash = Some("dash:999999".into());
        assert!(s.start(&r).err().unwrap().contains("no attached state"));
        scopes().detach(&n2);
    }
}
