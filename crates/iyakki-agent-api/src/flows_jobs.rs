//! The FLOWS panel split by job (F-SPLIT-03, D-S5-01): what the tile asks of the account's flows environment as five traits whose
//! methods carry data only, so each can cross the core socket (`FlowsMeta`, `FlowsDrafts`, `FlowsRuns`, `FlowsAssist`, `FlowsHooks`,
//! `FlowsChannel`). `FlowsJobs` is the whole of them for the tile; `FlowsPanel` (below) is the in-process facade that also hands out the
//! engine, the store and the host, which cannot cross.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_flow::assist_store::{Book, Global, Status, Suggestion};
use iyakki_flow::engine::{ApplyReport, ApplyScope, CardDecision, Counts, NodeTest, NodeUpdateInfo, StartOutcome, StartRequest};
use iyakki_flow::host::BrowserSessionInfo;
use iyakki_flow::kinds::Issue;
use iyakki_flow::model::{Flow, Run, RunSummary};
use iyakki_flow::store::{FlowMeta, FlowSummary, VersionEntry};
use iyakki_flowsx::hookview::{Created, HookOpts, HookView};
use iyakki_sec::policy::Policy;
use iyakki_shared::flows_channel_policy::{Held, Policy as MsgPolicy, Watch as MsgWatch};

/// Who the environment is for and where it keeps its files (answers read once by a remote end).
pub trait FlowsMeta: Send + Sync {
    fn user(&self) -> String;
    fn policy(&self) -> Policy;
    /// `<data>/flows/<user>`.
    fn data(&self) -> PathBuf;
    /// `<home>/Flows`.
    fn dir(&self) -> PathBuf;
    /// The services' data folder.
    fn svc_dir(&self) -> PathBuf;
    fn who(&self) -> String;
    /// A line in the core's audit log (action and detail, never a value), written as the engine's own lines are.
    fn audit(&self, action: &str, detail: &str);
    /// (id, name) of every flow as `flows.list` of the core contract answers it.
    fn flow_names(&self) -> Vec<(String, String)>;
    fn set_last_flow(&self, flow: &str, name: &str);
    /// This environment as the CHAT tile's handle.
    fn chat_handle(&self) -> Arc<dyn crate::FlowChatEnv>;
    /// Where a camera picture would go, in one line ("the vision model is ...").
    fn vision_line(&self) -> String;
    /// Ask every enabled provider for its models again; the answer in one line.
    fn refresh_model_lists(&self) -> String;
}

/// Flows as files: drafts, published versions, history, import and export.
pub trait FlowsDrafts: Send + Sync {
    fn summaries(&self) -> Vec<FlowSummary>;
    fn issues(&self, f: &Flow) -> Vec<Issue>;
    fn get_draft(&self, id: &str) -> Option<Flow>;
    fn published(&self, id: &str) -> Option<Flow>;
    fn versions(&self, id: &str) -> Vec<VersionEntry>;
    fn get_version(&self, id: &str, version: u32) -> Option<Flow>;
    fn meta_of(&self, id: &str) -> FlowMeta;
    /// A cheap change mark of a draft (size, change time in ns): the tile reads it to see that the file was written by someone else.
    fn draft_stamp(&self, id: &str) -> (u64, u128);
    fn rollback(&self, id: &str, to: u32, by: &str) -> Result<u32, String>;
    /// Publish the draft: the version number. `approved_by` is the person who looked at an agent's changes.
    fn publish(&self, id: &str, by: &str, label: &str, changelog: &str, approved_by: Option<String>) -> Result<u32, String>;
    fn save_edit(&self, flow: &Flow, by: &str, summary: &str, node: &str) -> Result<(), String>;
    fn create_draft(&self, f: Flow, by: &str) -> Result<Flow, String>;
    fn import_json(&self, text: &str, by: &str) -> Result<Flow, String>;
    fn export_json(&self, id: &str) -> Result<String, String>;
    fn from_template(&self, template: &str, name: &str, by: &str) -> Result<Flow, String>;
    fn allow_program(&self, name: &str) -> Result<(), String>;
    fn drop_version(&self, id: &str, version: u32) -> Result<(), String>;
    fn delete_flow(&self, id: &str) -> Result<String, String>;
}

/// Runs: start, watch, decide, cancel, retry, and the node updates.
pub trait FlowsRuns: Send + Sync {
    fn start_run(&self, req: StartRequest) -> Result<StartOutcome, String>;
    fn get_run(&self, id: &str) -> Option<Run>;
    fn run_summaries(&self) -> Vec<RunSummary>;
    fn run_counts(&self) -> Counts;
    /// Counts up on every change of any run: the tile reads it to see whether to read again (and the push `flows.subscribe` says when).
    fn revision(&self) -> u64;
    /// The scheduler is running.
    fn ticking(&self) -> bool;
    fn next_runs(&self, flow: &Flow, n: usize) -> Vec<(String, i64)>;
    fn decide(&self, run_id: &str, node: &str, approved: bool, by: &str, note: &str) -> Result<(), String>;
    fn card_decide(&self, run_id: &str, card: &str, d: CardDecision) -> Result<(), String>;
    fn cancel(&self, run_id: &str) -> Result<(), String>;
    fn retry_node(&self, run_id: &str, node: &str) -> Result<(), String>;
    fn skip_node(&self, run_id: &str, node: &str, by: &str) -> Result<(), String>;
    fn rerun_from(&self, run_id: &str, node: &str) -> Result<(), String>;
    fn node_updates(&self, flow_id: &str) -> Vec<NodeUpdateInfo>;
    fn apply_node_update(&self, flow_id: &str, node: &str, scope: ApplyScope, by: &str, allow_incompatible: bool) -> Result<ApplyReport, String>;
    /// Try one step with a sample input, without a run (it may take a while: run it off the UI thread).
    fn test_node(&self, flow: &Flow, node_id: &str, sample_inputs: serde_json::Value, input: serde_json::Value) -> Result<NodeTest, String>;
    fn report_run(&self, run_id: &str, flow_id: &str);
    /// The person takes back what a stopped run was allowed to ask for (browser permissions).
    fn withdraw_permissions(&self, run_id: &str) -> usize;
    /// The browser session a run has open, if any.
    fn browser_session(&self, run_id: &str) -> Option<BrowserSessionInfo>;
}

/// The assistant's book of suggestions, switches and watches, and what it does to a flow.
pub trait FlowsAssist: Send + Sync {
    fn assist_global(&self) -> Global;
    fn assist_set_enabled(&self, on: bool) -> Result<(), String>;
    fn assist_set_global_cap(&self, per_day: u32) -> Result<(), String>;
    fn assist_set_matrix(&self, on: bool, room: &str) -> Result<(), String>;
    fn assist_set_profile(&self, p: iyakki_flow::assist_profile::AgentProfile) -> Result<iyakki_flow::assist_profile::AgentProfile, String>;
    fn assist_book(&self, flow: &str) -> Book;
    fn assist_open_list(&self, flow: &str) -> Vec<Suggestion>;
    fn assist_now(&self) -> i64;
    fn assist_watch(&self, flow: &str) -> Option<iyakki_flow::assist_store::Watch>;
    fn assist_set_watch(&self, flow: &str, w: Option<iyakki_flow::assist_store::Watch>) -> Result<(), String>;
    fn assist_set_status(&self, flow: &str, sid: &str, st: Status) -> Result<Suggestion, String>;
    fn assist_paused(&self, flow: &str) -> bool;
    fn assist_set_paused(&self, flow: &str, on: bool) -> Result<(), String>;
    /// What the assistant did for a flow lately, as the screen shows it (`assist_activity::readout`).
    fn assist_readout(&self, flow: &str, now_ms: i64, limit: usize) -> serde_json::Value;
    fn assist_adopt(&self, flow: &str, sid: &str) -> Result<Suggestion, String>;
    fn assist_reject(&self, flow: &str, sid: &str, by: &str) -> Result<Suggestion, String>;
    fn trial_whole(&self, flow: &str) -> Result<iyakki_flow::assist_run::WholeTrial, String>;
    fn assist_after_publish(&self, flow: &str, version: u32);
}

/// Webhooks of flows (making one, renewing it, opening it to the public for a while are asks).
pub trait FlowsHooks: Send + Sync {
    fn hook_views(&self, flow: Option<&str>) -> Vec<HookView>;
    fn hook_create(&self, flow: &str, o: &HookOpts, by: &str) -> Result<Created, String>;
    fn hook_regenerate(&self, id: &str, by: &str) -> Result<Created, String>;
    fn hook_enable(&self, id: &str, by: &str) -> Result<(), String>;
    fn hook_disable(&self, id: &str, by: &str) -> Result<(), String>;
    fn hook_public(&self, id: &str, minutes: u64, by: &str) -> Result<i64, String>;
}

/// The outside-message policy (quiet hours, escalation) and the per-flow channel switch.
pub trait FlowsChannel: Send + Sync {
    fn chan_allowed(&self, flow: &str) -> bool;
    fn chan_set(&self, flow: &str, on: bool) -> Result<(), String>;
    fn msg_policy_get(&self) -> MsgPolicy;
    fn msg_policy_set(&self, p: MsgPolicy) -> Result<MsgPolicy, String>;
    fn msg_held(&self) -> Vec<Held>;
    fn msg_watching(&self) -> Vec<MsgWatch>;
    /// The person answered: stop watching for it; how many were watching.
    fn msg_ack(&self, flow: &str) -> usize;
}

/// All jobs of the tile in one object: the real environment, a fake, or the five remote objects of the UI program.
pub trait FlowsJobs: FlowsMeta + FlowsDrafts + FlowsRuns + FlowsAssist + FlowsHooks + FlowsChannel {
    /// The static catalog of step kinds (a leaf crate with no I/O; the tile links it for the types).
    fn reg(&self) -> Arc<iyakki_flow::kinds::Registry> {
        Arc::new(iyakki_flow::kinds::Registry::builtin())
    }
}

/// What the FLOWS panel really calls of the account's flows environment (the real one wraps `iyakki_agent::flows::FlowEnv`): the jobs
/// above, and what only the process that owns the engine can do: hand out the engine, the store and the host.
pub trait FlowsPanel: FlowsJobs {
    fn engine(&self) -> &iyakki_flow::engine::Engine;
    fn store(&self) -> &Arc<iyakki_flow::store::FlowStore>;
    /// The engine's host (audit lines, the run's open browser session).
    fn host(&self) -> &dyn iyakki_flow::host::Host;
    /// The outside-message policy book.
    fn msg_policy(&self) -> &iyakki_shared::flows_channel_policy::PolicyBook;
    fn assist_store(&self) -> iyakki_flow::assist_store::AssistStore;
    /// The environment itself, type-erased (the agent's own tests and the core contract take it back).
    fn as_any(&self) -> Arc<dyn std::any::Any + Send + Sync>;
}
