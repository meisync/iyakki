//! The Gemini voice session as the shell starts it (EXPERIMENTAL; docs/GEMINI-PROVIDER.md): builds the real parts for the signed-in
//! person from the shell's `AppCx` and hands them to `iyakki_agent::geminilive_host`. Everything that touches the machine goes
//! through the agent crate's own paths: the microphone through the Voice tile's capture path (`wakelisten::CaptureMic`), the sound
//! through the shared audio entry point (`SpeechSink`), the connection through `geminilive_ws`, the key through the vault.
//!
//! The tool set is the person's base chat tools with the SAME account chain the chat uses (`tool_chain::account_chain`: Settings, SSH, VPN,
//! cameras, documents, the model router, fleet ... all with the chat's grants and approvals). Only what needs a chat's own open state
//! (`tool_chain::CHAT_ONLY`: the open project's board, the flows and dashboard tiles, the task list) is not part of a voice session. Every call goes through the same wrappers, grants and approval cards:
//! a tool that asks puts a card on the shell's board and only a press answers it.

use std::sync::Arc;

use iyakki_shared::geminilive::{self};
use iyakki_agent_api::{LiveSpec};
use iyakki_shared::geminilive_host::{Board, Running};

use super::AppCx;

/// The tools of the account for a voice session.
pub fn voice_tools(cx: &AppCx) -> Arc<dyn iyakki_shared::agent::Tools> {
    let t: Arc<dyn iyakki_shared::agent::Tools> = cx.services.user_tools(&cx.session.user, cx.session.policy.clone(), cx.host.clone(), cx.mcp_servers.clone(), None, None);
    // the same account chain as the chat (F-GEMVOICE-15): same tools, same grants, same approval cards
    let t = super::tool_chain::account_chain(cx, t, &super::tool_chain::ChainScope::default());
    // the jobs and the pending-work ledger: a spoken "tell me when it is done" is kept like a typed one (no workspace, no task list)
    super::tool_chain::session_state_tools(cx, t, None, None, None)
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
