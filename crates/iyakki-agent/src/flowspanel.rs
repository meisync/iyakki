//! The real `FlowsPanel` (F-SPLIT-03, lane alpha.6/split-d): the account's `FlowEnv` as the FLOWS panel of the UI sees it.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_agent_api::{FlowChatEnv, FlowsAssist, FlowsChannel, FlowsDrafts, FlowsHooks, FlowsJobs, FlowsMeta, FlowsPanel, FlowsRuns, ServicesB6};
use iyakki_core_api::contract::{CallContext};
use iyakki_flow::assist_store::{AssistStore, Book, Global, Status, Suggestion};
use iyakki_flow::engine::{ApplyReport, ApplyScope, CardDecision, Counts, NodeTest, NodeUpdateInfo, StartOutcome, StartRequest};
use iyakki_flow::host::BrowserSessionInfo;
use iyakki_flow::model::{Run, RunSummary};
use iyakki_flow::store::{FlowMeta, VersionEntry};
use iyakki_flow::engine::Engine;
use iyakki_shared::flows_channel_policy::{Held, Policy as MsgPolicy, Watch as MsgWatch};
use iyakki_flow::kinds::{Issue, Registry};
use iyakki_flow::model::Flow;
use iyakki_flow::store::{FlowStore, FlowSummary};
use iyakki_flowsx::hookview::{Created, HookOpts, HookView};
use iyakki_sec::policy::Policy;

use crate::flows::FlowEnv;
use crate::services::Services;

/// The panel over one environment.
pub struct RealFlowsPanel(pub Arc<FlowEnv>);

/// The environment of the account, opened, or built without a scheduler when it cannot be opened.
pub fn open_panel(svc: &Arc<Services>, user: &str, policy: &Policy) -> Arc<dyn FlowsPanel> {
    let env = FlowEnv::open(svc, user, policy).unwrap_or_else(|_| Arc::new(FlowEnv::build(svc, user, policy, false)));
    Arc::new(RealFlowsPanel(env))
}

impl FlowsMeta for RealFlowsPanel {
    fn user(&self) -> String {
        self.0.user.clone()
    }
    fn policy(&self) -> Policy {
        self.0.policy.clone()
    }
    fn data(&self) -> PathBuf {
        self.0.data.clone()
    }
    fn dir(&self) -> PathBuf {
        self.0.dir.clone()
    }
    fn svc_dir(&self) -> PathBuf {
        self.0.svc.dir().to_path_buf()
    }
    fn who(&self) -> String {
        self.0.who()
    }
    fn audit(&self, action: &str, detail: &str) {
        iyakki_flow::host::Host::audit(self.0.host.as_ref(), action, detail);
    }
    fn flow_names(&self) -> Vec<(String, String)> {
        let ctx = CallContext::local(&self.0.user, self.0.policy.manage_settings);
        let env: Arc<dyn std::any::Any + Send + Sync> = self.0.clone();
        self.0.svc.contract_local().flows_for_env(env).and_then(|f| f.list(&ctx).ok()).map(|l| l.into_iter().map(|i| (i.id, i.name)).collect()).unwrap_or_default()
    }
    fn set_last_flow(&self, flow: &str, name: &str) {
        self.0.svc.flows_facet().set_last_flow(&self.0.user, flow, name);
    }
    fn chat_handle(&self) -> Arc<dyn FlowChatEnv> {
        self.0.chat_handle()
    }
    fn vision_line(&self) -> String {
        match self.0.svc.vision_model(None) {
            Ok(p) => p.line(),
            Err(e) => format!("No vision model is available yet: {}", e.message()),
        }
    }
    fn refresh_model_lists(&self) -> String {
        self.0.svc.refresh_model_lists()
    }
}

impl FlowsDrafts for RealFlowsPanel {
    fn summaries(&self) -> Vec<FlowSummary> {
        self.0.summaries()
    }
    fn issues(&self, f: &Flow) -> Vec<Issue> {
        self.0.issues(f)
    }
    fn get_draft(&self, id: &str) -> Option<Flow> {
        self.0.store.get_draft(id)
    }
    fn published(&self, id: &str) -> Option<Flow> {
        self.0.store.published(id)
    }
    fn versions(&self, id: &str) -> Vec<VersionEntry> {
        self.0.store.versions(id)
    }
    fn get_version(&self, id: &str, version: u32) -> Option<Flow> {
        self.0.store.get_version(id, version)
    }
    fn meta_of(&self, id: &str) -> FlowMeta {
        self.0.store.meta_of(id)
    }
    fn draft_stamp(&self, id: &str) -> (u64, u128) {
        let p = self.0.dir.join(id).join("flow.json");
        std::fs::metadata(p).map(|m| (m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos()))).unwrap_or((0, 0))
    }
    fn rollback(&self, id: &str, to: u32, by: &str) -> Result<u32, String> {
        self.0.store.rollback(id, to, by)
    }
    fn publish(&self, id: &str, by: &str, label: &str, changelog: &str, approved_by: Option<String>) -> Result<u32, String> {
        self.0.store.publish(id, &iyakki_flow::store::PublishReq { by, label, changelog, approved_by: approved_by.as_deref() })
    }
    fn save_edit(&self, flow: &Flow, by: &str, summary: &str, node: &str) -> Result<(), String> {
        self.0.save_edit(flow, by, summary, node)
    }
    fn create_draft(&self, f: Flow, by: &str) -> Result<Flow, String> {
        self.0.create_draft(f, by)
    }
    fn import_json(&self, text: &str, by: &str) -> Result<Flow, String> {
        self.0.import_json(text, by)
    }
    fn export_json(&self, id: &str) -> Result<String, String> {
        self.0.export_json(id)
    }
    fn from_template(&self, template: &str, name: &str, by: &str) -> Result<Flow, String> {
        self.0.from_template(template, name, by)
    }
    fn allow_program(&self, name: &str) -> Result<(), String> {
        self.0.allow_program(name)
    }
    fn drop_version(&self, id: &str, version: u32) -> Result<(), String> {
        self.0.drop_version(id, version)
    }
    fn delete_flow(&self, id: &str) -> Result<String, String> {
        self.0.delete_flow(id)
    }
}

impl FlowsRuns for RealFlowsPanel {
    fn start_run(&self, req: StartRequest) -> Result<StartOutcome, String> {
        self.0.engine.start_run(req)
    }
    fn get_run(&self, id: &str) -> Option<Run> {
        self.0.engine.get_run(id)
    }
    fn run_summaries(&self) -> Vec<RunSummary> {
        self.0.engine.summaries()
    }
    fn run_counts(&self) -> Counts {
        self.0.engine.counts()
    }
    fn revision(&self) -> u64 {
        self.0.engine.revision()
    }
    fn ticking(&self) -> bool {
        self.0.engine.ticking()
    }
    fn next_runs(&self, flow: &Flow, n: usize) -> Vec<(String, i64)> {
        self.0.engine.next_runs(flow, n)
    }
    fn decide(&self, run_id: &str, node: &str, approved: bool, by: &str, note: &str) -> Result<(), String> {
        self.0.engine.decide(run_id, node, approved, by, note)
    }
    fn card_decide(&self, run_id: &str, card: &str, d: CardDecision) -> Result<(), String> {
        self.0.engine.card_decide(run_id, card, d)
    }
    fn cancel(&self, run_id: &str) -> Result<(), String> {
        self.0.engine.cancel(run_id)
    }
    fn retry_node(&self, run_id: &str, node: &str) -> Result<(), String> {
        self.0.engine.retry_node(run_id, node)
    }
    fn skip_node(&self, run_id: &str, node: &str, by: &str) -> Result<(), String> {
        self.0.engine.skip_node(run_id, node, by)
    }
    fn rerun_from(&self, run_id: &str, node: &str) -> Result<(), String> {
        self.0.engine.rerun_from(run_id, node)
    }
    fn node_updates(&self, flow_id: &str) -> Vec<NodeUpdateInfo> {
        self.0.engine.node_updates(flow_id)
    }
    fn apply_node_update(&self, flow_id: &str, node: &str, scope: ApplyScope, by: &str, allow_incompatible: bool) -> Result<ApplyReport, String> {
        self.0.engine.apply_node_update(flow_id, node, scope, by, allow_incompatible)
    }
    fn test_node(&self, flow: &Flow, node_id: &str, sample_inputs: serde_json::Value, input: serde_json::Value) -> Result<NodeTest, String> {
        self.0.engine.test_node(flow, node_id, sample_inputs, input)
    }
    fn report_run(&self, run_id: &str, flow_id: &str) {
        self.0.report_run(run_id, flow_id)
    }
    fn withdraw_permissions(&self, run_id: &str) -> usize {
        self.0.svc.browser.consent.withdraw_permissions(&self.0.user, run_id)
    }
    fn browser_session(&self, run_id: &str) -> Option<BrowserSessionInfo> {
        iyakki_flow::host::Host::browser_session(self.0.host.as_ref(), run_id)
    }
}

impl FlowsAssist for RealFlowsPanel {
    fn assist_global(&self) -> Global {
        self.0.assist_store().global()
    }
    fn assist_set_enabled(&self, on: bool) -> Result<(), String> {
        self.0.assist_store().set_enabled(on)
    }
    fn assist_set_global_cap(&self, per_day: u32) -> Result<(), String> {
        self.0.assist_store().set_global_cap(per_day)
    }
    fn assist_set_matrix(&self, on: bool, room: &str) -> Result<(), String> {
        self.0.assist_store().set_matrix(on, room)
    }
    fn assist_set_profile(&self, p: iyakki_flow::assist_profile::AgentProfile) -> Result<iyakki_flow::assist_profile::AgentProfile, String> {
        self.0.assist_store().set_profile(p)
    }
    fn assist_book(&self, flow: &str) -> Book {
        self.0.assist_store().book(flow)
    }
    fn assist_open_list(&self, flow: &str) -> Vec<Suggestion> {
        self.0.assist_store().open_list(flow)
    }
    fn assist_now(&self) -> i64 {
        self.0.assist_store().now()
    }
    fn assist_watch(&self, flow: &str) -> Option<iyakki_flow::assist_store::Watch> {
        self.0.assist_store().watch(flow)
    }
    fn assist_set_watch(&self, flow: &str, w: Option<iyakki_flow::assist_store::Watch>) -> Result<(), String> {
        self.0.assist_store().set_watch(flow, w)
    }
    fn assist_set_status(&self, flow: &str, sid: &str, st: Status) -> Result<Suggestion, String> {
        self.0.assist_store().set_status(flow, sid, st)
    }
    fn assist_paused(&self, flow: &str) -> bool {
        self.0.assist_store().paused(flow)
    }
    fn assist_set_paused(&self, flow: &str, on: bool) -> Result<(), String> {
        self.0.assist_store().set_paused(flow, on)
    }
    fn assist_readout(&self, flow: &str, now_ms: i64, limit: usize) -> serde_json::Value {
        iyakki_flow::assist_activity::readout(&self.0.assist_store(), flow, now_ms, limit)
    }
    fn assist_adopt(&self, flow: &str, sid: &str) -> Result<Suggestion, String> {
        self.0.with_assistant(|a| a.adopt(flow, sid))
    }
    fn assist_reject(&self, flow: &str, sid: &str, by: &str) -> Result<Suggestion, String> {
        self.0.with_assistant(|a| a.reject(flow, sid, by))
    }
    fn trial_whole(&self, flow: &str) -> Result<iyakki_flow::assist_run::WholeTrial, String> {
        self.0.trial_whole(flow)
    }
    fn assist_after_publish(&self, flow: &str, version: u32) {
        self.0.assist_after_publish(flow, version)
    }
}

impl FlowsHooks for RealFlowsPanel {
    fn hook_views(&self, flow: Option<&str>) -> Vec<HookView> {
        self.0.hook_views(flow)
    }
    fn hook_create(&self, flow: &str, o: &HookOpts, by: &str) -> Result<Created, String> {
        self.0.hook_create(flow, o, by)
    }
    fn hook_regenerate(&self, id: &str, by: &str) -> Result<Created, String> {
        self.0.hook_regenerate(id, by)
    }
    fn hook_enable(&self, id: &str, by: &str) -> Result<(), String> {
        self.0.hook_enable(id, by)
    }
    fn hook_disable(&self, id: &str, by: &str) -> Result<(), String> {
        self.0.hook_disable(id, by)
    }
    fn hook_public(&self, id: &str, minutes: u64, by: &str) -> Result<i64, String> {
        self.0.hook_public(id, minutes, by)
    }
}

impl FlowsChannel for RealFlowsPanel {
    fn chan_allowed(&self, flow: &str) -> bool {
        self.0.host.chan.allowed(flow)
    }
    fn chan_set(&self, flow: &str, on: bool) -> Result<(), String> {
        self.0.host.chan.set(flow, on)
    }
    fn msg_policy_get(&self) -> MsgPolicy {
        self.0.host.msg_policy.policy()
    }
    fn msg_policy_set(&self, p: MsgPolicy) -> Result<MsgPolicy, String> {
        self.0.host.msg_policy.set_policy(p)
    }
    fn msg_held(&self) -> Vec<Held> {
        self.0.host.msg_policy.held()
    }
    fn msg_watching(&self) -> Vec<MsgWatch> {
        self.0.host.msg_policy.watching()
    }
    fn msg_ack(&self, flow: &str) -> usize {
        self.0.host.msg_policy.ack(flow)
    }
}

impl FlowsJobs for RealFlowsPanel {
    fn reg(&self) -> Arc<Registry> {
        self.0.reg.clone()
    }
}

impl FlowsPanel for RealFlowsPanel {
    fn engine(&self) -> &Engine {
        &self.0.engine
    }
    fn store(&self) -> &Arc<FlowStore> {
        &self.0.store
    }
    fn host(&self) -> &dyn iyakki_flow::host::Host {
        self.0.host.as_ref()
    }
    fn msg_policy(&self) -> &iyakki_shared::flows_channel_policy::PolicyBook {
        &self.0.host.msg_policy
    }
    fn assist_store(&self) -> AssistStore {
        self.0.assist_store()
    }
    fn as_any(&self) -> Arc<dyn std::any::Any + Send + Sync> {
        self.0.clone()
    }
}
