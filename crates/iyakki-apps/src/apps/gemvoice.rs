//! The Gemini voice session as the shell starts it (EXPERIMENTAL; docs/GEMINI-PROVIDER.md): builds the real parts for the signed-in
//! person from the shell's `AppCx` and hands them to `iyakki_agent::geminilive_host`. Everything that touches the machine goes
//! through the agent crate's own paths: the microphone through the Voice tile's capture path (`wakelisten::CaptureMic`), the sound
//! through the shared audio entry point (`SpeechSink`), the connection through `geminilive_ws`, the key through the vault.
//!
//! The tool set is the person's base chat tools with the SAME account chain the chat uses (`tool_chain::account_chain`: Settings, SSH, VPN,
//! cameras, documents, the model router, fleet ... all with the chat's grants and approvals). Only what needs a chat's own open state
//! (`tool_chain::CHAT_ONLY`, now empty) is not part of a voice session; the board, the parallel tasks of the project opened last, the dashboard opened last and the flow open last are (`voice_tools`). Every call goes through the same wrappers, grants and approval cards:
//! a tool that asks puts a card on the shell's board and only a press answers it.

use std::sync::Arc;

use iyakki_shared::geminilive::{self};
use iyakki_agent_api::{LiveSpec};
use iyakki_shared::geminilive_host::{Board, Running};

use super::AppCx;

/// The workspace a spoken request means when it names none: the one the person opened last in the IDE (D-GEMVOICE-40-NAMING), the same
/// "last one open" rule the CHAT tile follows for a flow. `None` when the person has no workspace or its folder is gone.
pub fn last_workspace(cx: &AppCx) -> Option<super::chat::ProjectScope> {
    let reg = iyakki_shared::workspace::Registry::load(cx.data_dir(), &cx.session.user);
    let w = reg.last.as_deref().and_then(|id| reg.get(id)).or_else(|| reg.items.iter().max_by_key(|w| w.last_opened))?;
    let root = std::path::PathBuf::from(&w.root);
    root.is_dir().then(|| super::chat::ProjectScope { name: w.name.clone(), root: root.clone(), key: format!("ide:{}", w.name), features: root.join("FEATURES.md"), cluster: None })
}

/// The tools of the account for a voice session: the account chain of the chat, plus the open-state tools a spoken request needs a name
/// for. By the default of D-GEMVOICE-40-NAMING a request that names no project, flow, dashboard or task list means the project the person
/// opened last (its board tools, the same grants and cards as in the chat) and the flow they had open last; the dashboard tools and the task
/// tools stay chat-only because each belongs to a tile that is open on the screen.
pub fn voice_tools(cx: &AppCx) -> Arc<dyn iyakki_shared::agent::Tools> {
    let policy = cx.session.policy.clone();
    let project = (policy.cli == iyakki_sec::policy::CliLevel::Full).then(|| last_workspace(cx)).flatten();
    let t: Arc<dyn iyakki_shared::agent::Tools> = cx.services.user_tools(&cx.session.user, policy, cx.host.clone(), cx.mcp_servers.clone(), None, None);
    let t = match &project {
        Some(p) => {
            let mut opts = iyakki_shared::context::ContextOptions { use_docs: false, ..Default::default() };
            let pp = super::core_run::project_parts(cx, p, None, false, false, "", &mut opts);
            super::core_run::wrap_project_tools(cx, t, pp.policy.clone(), p, pp.kit.as_ref(), pp.platform)
        }
        None => t,
    };
    // the parallel tasks of that workspace: the SAME manager the IDE's Tasks tab holds while it is open (full terminal accounts only)
    let tasks = project.as_ref().filter(|_| policy_full(cx)).map(|p| super::ide_tasks_run::shared_manager(cx, &p.root, &p.features, &p.name));
    let t = match &tasks {
        Some(m) => cx.tasks_facet().wrap_task_tools(t, m, "builtin", ""),
        None => t,
    };
    // the dashboard the person opened last (the open-dashboard tools bind to it; a change is kept only by `dash_save`, which asks)
    let t = match last_dashboard(cx) {
        Some((env, state)) => Arc::new(iyakki_dash::tools::DashTools::new(t, env, state)) as Arc<dyn iyakki_shared::agent::Tools>,
        None => t,
    };
    // the flow the person had open last, when there is one (the CHAT tile's own rule)
    let user = cx.session.user.as_str();
    let flow = cx.core.flows_view().last_flow_recorded(user).then(|| cx.flows_facet().open(user, &cx.session.policy).ok()).flatten().and_then(|env| env.last_flow().map(|(id, _)| (env, id)));
    let t = match flow {
        Some((env, id)) => env.wrap_tools(t, std::sync::Arc::new(std::sync::Mutex::new(iyakki_flowsx::spec::Open { flow: Some(id), tab: "design".into(), ..Default::default() }))),
        None => t,
    };
    // the same account chain as the chat (F-GEMVOICE-15): same tools, same grants, same approval cards
    let t = super::tool_chain::account_chain(cx, t, &super::tool_chain::ChainScope { project: project.as_ref().map(|p| (p.name.clone(), p.root.clone())), ext: None });
    // the jobs and the pending-work ledger: a spoken "tell me when it is done" is kept like a typed one (no task list)
    super::tool_chain::session_state_tools(cx, t, project.as_ref().map(|p| p.root.clone()), project.as_ref().map(|p| (p.name.as_str(), p.root.as_path())), tasks)
}

fn policy_full(cx: &AppCx) -> bool {
    cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full
}

/// The dashboard the person opened last in DASH with the environment its tools need, for an account that may use dashboards.
pub fn last_dashboard(cx: &AppCx) -> Option<(Arc<iyakki_dash::tools::Env>, iyakki_dash::session::Shared)> {
    if !iyakki_dash::tools::offered(&cx.session.policy) {
        return None;
    }
    let host = cx.services.dash_host();
    let mut env = iyakki_dash::tools::Env::from_host_in(&host, &cx.session.user, &cx.session.policy, host.home_root(&cx.session.user));
    let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
    env.audit = Arc::new(move |kind, detail| audit.record(&user, kind, detail));
    let d = env.dashboards().last_opened()?;
    Some((Arc::new(env), iyakki_dash::session::shared(d)))
}

/// What tests put in place of the machine's parts (the shell's tests run a whole manual turn against a fake live server).
#[cfg(any(test, feature = "testing"))]
#[derive(Clone, Default)]
pub struct TestParts {
    pub connector: Option<Arc<dyn geminilive::LiveConnector>>,
    pub mic: Option<Arc<dyn Fn() -> Box<dyn iyakki_shared::wakelisten::Mic> + Send + Sync>>,
    pub sink: Option<Arc<dyn iyakki_shared::story::Sink>>,
    pub speaker: Option<Arc<dyn Fn() -> Box<dyn iyakki_shared::voiceassist::Speaker> + Send + Sync>>,
    pub tools: Option<Arc<dyn iyakki_shared::agent::Tools>>,
    pub key: Option<String>,
}

#[cfg(any(test, feature = "testing"))]
static TEST_PARTS: std::sync::Mutex<Option<TestParts>> = std::sync::Mutex::new(None);

/// Tests: replace the real parts (`None` puts the real ones back).
#[cfg(any(test, feature = "testing"))]
pub fn set_test_parts(p: Option<TestParts>) {
    *TEST_PARTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = p;
}

/// The parts of a session for `cx`'s person. `board` is the shell's board of cards. The connection, the key, the sound and the speaker
/// question are the core's (`GeminiLive::spawn`).
pub fn spec(cx: &AppCx, board: Arc<Board>) -> LiveSpec {
    let svc = cx.services.clone();
    let user = cx.session.user.clone();
    let audit = cx.audit.clone();
    let audit_user = user.clone();
    #[allow(unused_mut)]
    let mut p = LiveSpec {
        user: user.clone(),
        tools: if cx.session.policy.agent { Some(voice_tools(cx)) } else { None },
        board,
        audit: Arc::new(move |a, d| audit.record(&audit_user, a, d)),
        // a voice session never auto-approves: every tool that asks puts its card on the screen
        auto: Arc::new(|| false),
        mic: svc.wake_capture_mic(),
        speaker: svc.system_speaker(),
        log: svc.voice_turn_log(&user),
        connector: None,
        sink: None,
        key: None,
    };
    #[cfg(any(test, feature = "testing"))]
    if let Some(t) = TEST_PARTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone() {
        if let Some(c) = t.connector {
            p.connector = Some(Box::new(c));
        }
        if let Some(m) = t.mic {
            p.mic = m();
        }
        if let Some(k) = t.sink {
            p.sink = Some(k);
        }
        if let Some(sp) = t.speaker {
            p.speaker = sp();
        }
        if let Some(tl) = t.tools {
            p.tools = Some(tl);
        }
        if let Some(k) = t.key {
            p.key = Some(k);
        }
    }
    p
}

/// Starts a manual session (the Super+V key, the microphone button). Returns at once; the state is read from the handle.
pub fn start(cx: &AppCx, board: Arc<Board>) -> Running {
    cx.audit.record(&cx.session.user, "voice.session", "manual start asked (EXPERIMENTAL)");
    cx.services.gemini_live().spawn(spec(cx, board))
}

/// Whether the key and the button are offered to this person: the assistant is switched on and they may use voice.
pub fn offered(cx: &AppCx) -> bool {
    (cx.session.policy.voice || cx.session.policy.manage_settings) && cx.agent.b0().gemini().voice_enabled(cx.data_dir())
}

/// The start key's name from the settings (`Super+V`), for the key handler and the hints.
pub fn key_name(cx: &AppCx) -> String {
    cx.agent.b0().gemini().voice_key(cx.data_dir())
}

pub use geminilive::EndReason;
