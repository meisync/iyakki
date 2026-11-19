//! The real `FlowsPanel` (F-SPLIT-03, lane alpha.6/split-d): the account's `FlowEnv` as the FLOWS panel of the UI sees it.

use std::path::Path;
use std::sync::Arc;

use iyakki_agent_api::{FlowChatEnv, FlowsPanel, ServicesB6};
use iyakki_core_api::contract::{CallContext};
use iyakki_flow::assist_store::{AssistStore, Suggestion};
use iyakki_flow::engine::Engine;
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

impl FlowsPanel for RealFlowsPanel {
    fn user(&self) -> &str {
        &self.0.user
    }
    fn policy(&self) -> &Policy {
        &self.0.policy
    }
    fn data(&self) -> &Path {
        &self.0.data
    }
    fn dir(&self) -> &Path {
        &self.0.dir
    }
    fn svc_dir(&self) -> &Path {
        self.0.svc.dir()
    }
    fn engine(&self) -> &Engine {
        &self.0.engine
    }
    fn store(&self) -> &Arc<FlowStore> {
        &self.0.store
    }
    fn reg(&self) -> Arc<Registry> {
        self.0.reg.clone()
    }
    fn host(&self) -> &dyn iyakki_flow::host::Host {
        self.0.host.as_ref()
    }
    fn msg_policy(&self) -> &iyakki_shared::flows_channel_policy::PolicyBook {
        &self.0.host.msg_policy
    }
    fn chan_allowed(&self, flow: &str) -> bool {
        self.0.host.chan.allowed(flow)
    }
    fn chan_set(&self, flow: &str, on: bool) -> Result<(), String> {
        self.0.host.chan.set(flow, on)
    }
    fn set_audit_sink(&self, f: Arc<dyn Fn(&str, &str, &str) + Send + Sync>) {
        *self.0.host.audit_sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(f);
    }
    fn set_js(&self, f: Arc<dyn Fn(&str, &serde_json::Value, u64) -> Result<serde_json::Value, String> + Send + Sync>) {
        *self.0.host.js.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(f);
    }
    fn who(&self) -> String {
        self.0.who()
    }
    fn summaries(&self) -> Vec<FlowSummary> {
        self.0.summaries()
    }
    fn issues(&self, f: &Flow) -> Vec<Issue> {
        self.0.issues(f)
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
    fn report_run(&self, run_id: &str, flow_id: &str) {
        self.0.report_run(run_id, flow_id)
    }
    fn assist_after_publish(&self, flow: &str, version: u32) {
        self.0.assist_after_publish(flow, version)
    }
    fn assist_store(&self) -> AssistStore {
        self.0.assist_store()
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
    fn withdraw_permissions(&self, run_id: &str) -> usize {
        self.0.svc.browser.consent.withdraw_permissions(&self.0.user, run_id)
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
    fn as_any(&self) -> Arc<dyn std::any::Any + Send + Sync> {
        self.0.clone()
    }
}
