//! Facets of alpha.6 lane split-d (F-SPLIT-03): what the chat and the model-host panel needed from the services, and the system
//! microphones and speaker the composition root installs in tests. The real implementation is `iyakki_agent::facets_b7`; the
//! defaults here are the fake (a core that serves none of them).

use std::path::Path;
use std::sync::Arc;

use iyakki_shared::chat::Message;
use iyakki_shared::context::{ContextInfo, ContextOptions};
use iyakki_shared::modelhost::{Host, NewHost, TestReport};
use iyakki_shared::hostvoice::HostVoice;
use iyakki_shared::providers::ProviderCfg;
use iyakki_shared::story::{Sink, Synth, TtsStatus};
use iyakki_shared::storyvoice::QuickEngine;
use iyakki_shared::voice::{SpeakOpts, SpeechEngine, VoiceSettings};
use iyakki_shared::voicedesk::{Players, Recorders};

/// The answer of a call this core does not serve.
pub const UNAVAILABLE_B7: &str = "unavailable: this core does not serve this call";

/// What needs the process's services; implemented for `iyakki_agent::services::Services`.
pub trait ServicesB7 {
    /// The messages a chat turn sends to the model: the system prompt with the recalled memory and skills, the person's history
    /// condensed to the window, and the question (it may search the knowledge base and call an embedding service: run it off the UI thread).
    #[allow(clippy::too_many_arguments)]
    fn chat_context(&self, _user: &str, _session_id: Option<&str>, base_system: &str, history: &[Message], user_msg: &str, _opts: &ContextOptions) -> (Vec<Message>, ContextInfo) {
        let mut out = vec![Message::system(base_system)];
        out.extend(history.iter().cloned());
        out.push(Message::user(user_msg));
        (out, ContextInfo::default())
    }
    /// Register a tested model host in Models (provider `mac-<nickname>`, its key in the system vault) and keep it in the host list.
    fn modelhost_add(&self, _n: &NewHost, _report: &TestReport) -> Result<Host, String> {
        Err(UNAVAILABLE_B7.into())
    }
    /// The text-to-speech engine of Settings -> Voice with the voice and speed picked on the page, as the reader of a story, a chat answer or a lesson uses it.
    fn reading_synth(self: &Arc<Self>, _opts: SpeakOpts) -> Arc<dyn Synth> {
        Arc::new(NoSynth)
    }
    /// The FLOWS environment of the account (the one the engine runs in): the open one, or one built without a scheduler.
    fn flows_panel(self: &Arc<Self>, _user: &str, _policy: &iyakki_sec::policy::Policy) -> Option<Arc<dyn FlowsPanel>> {
        None
    }
    /// Take a model host out of Models and the list.
    fn modelhost_remove(&self, _id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_B7.into())
    }
}

/// A voice that makes no sound (a core without speech).
pub struct NoSynth;
impl Synth for NoSynth {
    fn synth(&self, _text: &str) -> Result<Vec<u8>, String> {
        Err(UNAVAILABLE_B7.into())
    }
}

/// The system microphones and speaker of this machine (the real ones are in `iyakki_agent::voicedesk`).
pub trait HelpersB7: Send + Sync {
    /// The microphones and recordings of the system (arecord and the mixer).
    fn system_recorders(&self) -> Option<Arc<dyn Recorders>> {
        None
    }
    /// The sound sink that plays WAV bytes through the play command of Settings -> Voice, streamed.
    fn command_sink(&self, _play_cmd: &str) -> Arc<dyn Sink> {
        Arc::new(crate::NoSink)
    }
    /// What reading aloud needs now: the engine, the front (screen) and the player, in words (`auto` is the engine this computer found on its own).
    fn tts_status(&self, _v: &VoiceSettings, _voice_allowed: bool, _path: &str, _host: &HostVoice, _auto: Option<&SpeechEngine>) -> TtsStatus {
        TtsStatus::NoEngine
    }
    /// A status as one plain sentence.
    fn tts_status_text(&self, _s: &TtsStatus) -> String {
        String::new()
    }
    /// The one-click speech set-ups the providers and the computer offer.
    fn quick_engines(&self, _providers: &[ProviderCfg], _path: &str, _voice_dirs: &[std::path::PathBuf]) -> Vec<QuickEngine> {
        Vec::new()
    }
    /// The speaker of the system (aplay and the mixer).
    fn system_players(&self) -> Option<Arc<dyn Players>> {
        None
    }
}

/// The default helpers.
pub struct NoB7;
impl HelpersB7 for NoB7 {}

/// The accessor (a supertrait of `AgentFacets`).
pub trait AgentFacetsB7: Send + Sync {
    fn helpers_b7(&self) -> &dyn HelpersB7 {
        &NoB7
    }
}
impl AgentFacetsB7 for crate::FakeAgentApi {}

/// What the FLOWS panel really calls of the account's flows environment (the real one wraps `iyakki_agent::flows::FlowEnv`): the engine,
/// the store and the registry are the leaf crate's own types; everything that needs the services or the agent's host is a method here.
pub trait FlowsPanel: Send + Sync {
    fn user(&self) -> &str;
    fn policy(&self) -> &iyakki_sec::policy::Policy;
    /// `<data>/flows/<user>`.
    fn data(&self) -> &Path;
    /// `<home>/Flows`.
    fn dir(&self) -> &Path;
    /// The services' data folder.
    fn svc_dir(&self) -> &Path;
    fn engine(&self) -> &iyakki_flow::engine::Engine;
    fn store(&self) -> &Arc<iyakki_flow::store::FlowStore>;
    fn reg(&self) -> Arc<iyakki_flow::kinds::Registry>;
    /// The engine's host (audit lines, the run's open browser session).
    fn host(&self) -> &dyn iyakki_flow::host::Host;
    /// The outside-message policy book (quiet hours, escalation) and the per-flow channel switch.
    fn msg_policy(&self) -> &iyakki_shared::flows_channel_policy::PolicyBook;
    fn chan_allowed(&self, flow: &str) -> bool;
    fn chan_set(&self, flow: &str, on: bool) -> Result<(), String>;
    /// Where what the engine does goes in the audit log, and the script runner (the in-house JavaScript engine of the UI).
    fn set_audit_sink(&self, f: Arc<dyn Fn(&str, &str, &str) + Send + Sync>);
    fn set_js(&self, f: Arc<dyn Fn(&str, &serde_json::Value, u64) -> Result<serde_json::Value, String> + Send + Sync>);
    fn who(&self) -> String;
    fn summaries(&self) -> Vec<iyakki_flow::store::FlowSummary>;
    fn issues(&self, f: &iyakki_flow::model::Flow) -> Vec<iyakki_flow::kinds::Issue>;
    fn save_edit(&self, flow: &iyakki_flow::model::Flow, by: &str, summary: &str, node: &str) -> Result<(), String>;
    fn create_draft(&self, f: iyakki_flow::model::Flow, by: &str) -> Result<iyakki_flow::model::Flow, String>;
    fn import_json(&self, text: &str, by: &str) -> Result<iyakki_flow::model::Flow, String>;
    fn export_json(&self, id: &str) -> Result<String, String>;
    fn from_template(&self, template: &str, name: &str, by: &str) -> Result<iyakki_flow::model::Flow, String>;
    fn allow_program(&self, name: &str) -> Result<(), String>;
    fn drop_version(&self, id: &str, version: u32) -> Result<(), String>;
    fn delete_flow(&self, id: &str) -> Result<String, String>;
    fn report_run(&self, run_id: &str, flow_id: &str);
    fn assist_after_publish(&self, flow: &str, version: u32);
    fn assist_store(&self) -> iyakki_flow::assist_store::AssistStore;
    fn assist_adopt(&self, flow: &str, sid: &str) -> Result<iyakki_flow::assist_store::Suggestion, String>;
    fn assist_reject(&self, flow: &str, sid: &str, by: &str) -> Result<iyakki_flow::assist_store::Suggestion, String>;
    fn trial_whole(&self, flow: &str) -> Result<iyakki_flow::assist_run::WholeTrial, String>;
    fn hook_views(&self, flow: Option<&str>) -> Vec<iyakki_flowsx::hookview::HookView>;
    fn hook_create(&self, flow: &str, o: &iyakki_flowsx::hookview::HookOpts, by: &str) -> Result<iyakki_flowsx::hookview::Created, String>;
    fn hook_regenerate(&self, id: &str, by: &str) -> Result<iyakki_flowsx::hookview::Created, String>;
    fn hook_enable(&self, id: &str, by: &str) -> Result<(), String>;
    fn hook_disable(&self, id: &str, by: &str) -> Result<(), String>;
    fn hook_public(&self, id: &str, minutes: u64, by: &str) -> Result<i64, String>;
    /// The person takes back what a stopped run was allowed to ask for (browser permissions).
    fn withdraw_permissions(&self, run_id: &str) -> usize;
    /// Where a camera picture would go, in one line ("the vision model is ...").
    fn vision_line(&self) -> String;
    /// Ask every enabled provider for its models again; the answer in one line.
    fn refresh_model_lists(&self) -> String;
    /// (id, name) of every flow as `flows.list` of the core contract answers it.
    fn flow_names(&self) -> Vec<(String, String)>;
    fn set_last_flow(&self, flow: &str, name: &str);
    /// This environment as the CHAT tile's handle.
    fn chat_handle(&self) -> Arc<dyn crate::FlowChatEnv>;
    /// The environment itself, type-erased (the agent's own tests and the core contract take it back).
    fn as_any(&self) -> Arc<dyn std::any::Any + Send + Sync>;
}
