//! The real `FlowsPort` of the FLOWS tile (alpha.8 N1): the account's flows jobs (`iyakki_agent_api::FlowsJobs`, in this process the real
//! environment, over the core socket the remote objects) behind the model crate's traits. Every method forwards one call; the data types are the
//! same, so nothing is converted. Written once from the six job traits (the generated socket code reads those and is untouched); a method added
//! to a job trait is added here and in `iyakki-flows-model/src/port.rs`, and the test `every_job_method_is_forwarded` says when one is missing.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_agent_api::{FlowsAssist, FlowsChannel, FlowsDrafts, FlowsHooks, FlowsJobs, FlowsMeta, FlowsRuns};
use iyakki_flow::assist_store::{Book, Global, Status, Suggestion};
use iyakki_flow::engine::{ApplyReport, ApplyScope, CardDecision, Counts, NodeTest, NodeUpdateInfo, StartOutcome, StartRequest};
use iyakki_flow::host::BrowserSessionInfo;
use iyakki_flow::kinds::Issue;
use iyakki_flow::model::{Flow, Run, RunSummary};
use iyakki_flow::store::{FlowMeta, FlowSummary, VersionEntry};
use iyakki_flows_model::channel::{Held, Policy as MsgPolicy, Watch as MsgWatch};
use iyakki_flows_model::hookview::{Created, HookOpts, HookView};
use iyakki_flows_model::port::{FlowsAssistPort, FlowsChannelPort, FlowsDraftsPort, FlowsHooksPort, FlowsMetaPort, FlowsPort, FlowsRunsPort};
use iyakki_sec::policy::Policy;

/// The jobs of the account, as the tile's port.
pub struct JobsPort(pub Arc<dyn FlowsJobs>);

impl FlowsMetaPort for JobsPort {
    fn user(&self) -> String {
        FlowsMeta::user(&*self.0)
    }
    fn policy(&self) -> Policy {
        FlowsMeta::policy(&*self.0)
    }
    fn data(&self) -> PathBuf {
        FlowsMeta::data(&*self.0)
    }
    fn dir(&self) -> PathBuf {
        FlowsMeta::dir(&*self.0)
    }
    fn svc_dir(&self) -> PathBuf {
        FlowsMeta::svc_dir(&*self.0)
    }
    fn who(&self) -> String {
        FlowsMeta::who(&*self.0)
    }
    fn audit(&self, action: &str, detail: &str) {
        FlowsMeta::audit(&*self.0, action, detail)
    }
    fn flow_names(&self) -> Vec<(String, String)> {
        FlowsMeta::flow_names(&*self.0)
    }
    fn set_last_flow(&self, flow: &str, name: &str) {
        FlowsMeta::set_last_flow(&*self.0, flow, name)
    }
    fn vision_line(&self) -> String {
        FlowsMeta::vision_line(&*self.0)
    }
    fn refresh_model_lists(&self) -> String {
        FlowsMeta::refresh_model_lists(&*self.0)
    }
}

impl FlowsDraftsPort for JobsPort {
    fn summaries(&self) -> Vec<FlowSummary> {
        FlowsDrafts::summaries(&*self.0)
    }
    fn issues(&self, f: &Flow) -> Vec<Issue> {
        FlowsDrafts::issues(&*self.0, f)
    }
    fn get_draft(&self, id: &str) -> Option<Flow> {
        FlowsDrafts::get_draft(&*self.0, id)
    }
    fn published(&self, id: &str) -> Option<Flow> {
        FlowsDrafts::published(&*self.0, id)
    }
    fn versions(&self, id: &str) -> Vec<VersionEntry> {
        FlowsDrafts::versions(&*self.0, id)
    }
    fn get_version(&self, id: &str, version: u32) -> Option<Flow> {
        FlowsDrafts::get_version(&*self.0, id, version)
    }
    fn meta_of(&self, id: &str) -> FlowMeta {
        FlowsDrafts::meta_of(&*self.0, id)
    }
    fn draft_stamp(&self, id: &str) -> (u64, u128) {
        FlowsDrafts::draft_stamp(&*self.0, id)
    }
    fn rollback(&self, id: &str, to: u32, by: &str) -> Result<u32, String> {
        FlowsDrafts::rollback(&*self.0, id, to, by)
    }
    fn publish(&self, id: &str, by: &str, label: &str, changelog: &str, approved_by: Option<String>) -> Result<u32, String> {
        FlowsDrafts::publish(&*self.0, id, by, label, changelog, approved_by)
    }
    fn save_edit(&self, flow: &Flow, by: &str, summary: &str, node: &str) -> Result<(), String> {
        FlowsDrafts::save_edit(&*self.0, flow, by, summary, node)
    }
    fn create_draft(&self, f: Flow, by: &str) -> Result<Flow, String> {
        FlowsDrafts::create_draft(&*self.0, f, by)
    }
    fn import_json(&self, text: &str, by: &str) -> Result<Flow, String> {
        FlowsDrafts::import_json(&*self.0, text, by)
    }
    fn export_json(&self, id: &str) -> Result<String, String> {
        FlowsDrafts::export_json(&*self.0, id)
    }
    fn from_template(&self, template: &str, name: &str, by: &str) -> Result<Flow, String> {
        FlowsDrafts::from_template(&*self.0, template, name, by)
    }
    fn allow_program(&self, name: &str) -> Result<(), String> {
        FlowsDrafts::allow_program(&*self.0, name)
    }
    fn drop_version(&self, id: &str, version: u32) -> Result<(), String> {
        FlowsDrafts::drop_version(&*self.0, id, version)
    }
    fn delete_flow(&self, id: &str) -> Result<String, String> {
        FlowsDrafts::delete_flow(&*self.0, id)
    }
}

impl FlowsRunsPort for JobsPort {
    fn start_run(&self, req: StartRequest) -> Result<StartOutcome, String> {
        FlowsRuns::start_run(&*self.0, req)
    }
    fn get_run(&self, id: &str) -> Option<Run> {
        FlowsRuns::get_run(&*self.0, id)
    }
    fn run_summaries(&self) -> Vec<RunSummary> {
        FlowsRuns::run_summaries(&*self.0)
    }
    fn run_counts(&self) -> Counts {
        FlowsRuns::run_counts(&*self.0)
    }
    fn revision(&self) -> u64 {
        FlowsRuns::revision(&*self.0)
    }
    fn ticking(&self) -> bool {
        FlowsRuns::ticking(&*self.0)
    }
    fn next_runs(&self, flow: &Flow, n: usize) -> Vec<(String, i64)> {
        FlowsRuns::next_runs(&*self.0, flow, n)
    }
    fn decide(&self, run_id: &str, node: &str, approved: bool, by: &str, note: &str) -> Result<(), String> {
        FlowsRuns::decide(&*self.0, run_id, node, approved, by, note)
    }
    fn card_decide(&self, run_id: &str, card: &str, d: CardDecision) -> Result<(), String> {
        FlowsRuns::card_decide(&*self.0, run_id, card, d)
    }
    fn cancel(&self, run_id: &str) -> Result<(), String> {
        FlowsRuns::cancel(&*self.0, run_id)
    }
    fn retry_node(&self, run_id: &str, node: &str) -> Result<(), String> {
        FlowsRuns::retry_node(&*self.0, run_id, node)
    }
    fn skip_node(&self, run_id: &str, node: &str, by: &str) -> Result<(), String> {
        FlowsRuns::skip_node(&*self.0, run_id, node, by)
    }
    fn rerun_from(&self, run_id: &str, node: &str) -> Result<(), String> {
        FlowsRuns::rerun_from(&*self.0, run_id, node)
    }
    fn node_updates(&self, flow_id: &str) -> Vec<NodeUpdateInfo> {
        FlowsRuns::node_updates(&*self.0, flow_id)
    }
    fn apply_node_update(&self, flow_id: &str, node: &str, scope: ApplyScope, by: &str, allow_incompatible: bool) -> Result<ApplyReport, String> {
        FlowsRuns::apply_node_update(&*self.0, flow_id, node, scope, by, allow_incompatible)
    }
    fn test_node(&self, flow: &Flow, node_id: &str, sample_inputs: serde_json::Value, input: serde_json::Value) -> Result<NodeTest, String> {
        FlowsRuns::test_node(&*self.0, flow, node_id, sample_inputs, input)
    }
    fn report_run(&self, run_id: &str, flow_id: &str) {
        FlowsRuns::report_run(&*self.0, run_id, flow_id)
    }
    fn withdraw_permissions(&self, run_id: &str) -> usize {
        FlowsRuns::withdraw_permissions(&*self.0, run_id)
    }
    fn browser_session(&self, run_id: &str) -> Option<BrowserSessionInfo> {
        FlowsRuns::browser_session(&*self.0, run_id)
    }
}

impl FlowsAssistPort for JobsPort {
    fn assist_global(&self) -> Global {
        FlowsAssist::assist_global(&*self.0)
    }
    fn assist_set_enabled(&self, on: bool) -> Result<(), String> {
        FlowsAssist::assist_set_enabled(&*self.0, on)
    }
    fn assist_set_global_cap(&self, per_day: u32) -> Result<(), String> {
        FlowsAssist::assist_set_global_cap(&*self.0, per_day)
    }
    fn assist_set_matrix(&self, on: bool, room: &str) -> Result<(), String> {
        FlowsAssist::assist_set_matrix(&*self.0, on, room)
    }
    fn assist_set_profile(&self, p: iyakki_flow::assist_profile::AgentProfile) -> Result<iyakki_flow::assist_profile::AgentProfile, String> {
        FlowsAssist::assist_set_profile(&*self.0, p)
    }
    fn assist_book(&self, flow: &str) -> Book {
        FlowsAssist::assist_book(&*self.0, flow)
    }
    fn assist_open_list(&self, flow: &str) -> Vec<Suggestion> {
        FlowsAssist::assist_open_list(&*self.0, flow)
    }
    fn assist_now(&self) -> i64 {
        FlowsAssist::assist_now(&*self.0)
    }
    fn assist_watch(&self, flow: &str) -> Option<iyakki_flow::assist_store::Watch> {
        FlowsAssist::assist_watch(&*self.0, flow)
    }
    fn assist_set_watch(&self, flow: &str, w: Option<iyakki_flow::assist_store::Watch>) -> Result<(), String> {
        FlowsAssist::assist_set_watch(&*self.0, flow, w)
    }
    fn assist_set_status(&self, flow: &str, sid: &str, st: Status) -> Result<Suggestion, String> {
        FlowsAssist::assist_set_status(&*self.0, flow, sid, st)
    }
    fn assist_paused(&self, flow: &str) -> bool {
        FlowsAssist::assist_paused(&*self.0, flow)
    }
    fn assist_set_paused(&self, flow: &str, on: bool) -> Result<(), String> {
        FlowsAssist::assist_set_paused(&*self.0, flow, on)
    }
    fn assist_readout(&self, flow: &str, now_ms: i64, limit: usize) -> serde_json::Value {
        FlowsAssist::assist_readout(&*self.0, flow, now_ms, limit)
    }
    fn assist_adopt(&self, flow: &str, sid: &str) -> Result<Suggestion, String> {
        FlowsAssist::assist_adopt(&*self.0, flow, sid)
    }
    fn assist_reject(&self, flow: &str, sid: &str, by: &str) -> Result<Suggestion, String> {
        FlowsAssist::assist_reject(&*self.0, flow, sid, by)
    }
    fn trial_whole(&self, flow: &str) -> Result<iyakki_flow::assist_run::WholeTrial, String> {
        FlowsAssist::trial_whole(&*self.0, flow)
    }
    fn assist_after_publish(&self, flow: &str, version: u32) {
        FlowsAssist::assist_after_publish(&*self.0, flow, version)
    }
}

impl FlowsHooksPort for JobsPort {
    fn hook_views(&self, flow: Option<&str>) -> Vec<HookView> {
        FlowsHooks::hook_views(&*self.0, flow)
    }
    fn hook_create(&self, flow: &str, o: &HookOpts, by: &str) -> Result<Created, String> {
        FlowsHooks::hook_create(&*self.0, flow, o, by)
    }
    fn hook_regenerate(&self, id: &str, by: &str) -> Result<Created, String> {
        FlowsHooks::hook_regenerate(&*self.0, id, by)
    }
    fn hook_enable(&self, id: &str, by: &str) -> Result<(), String> {
        FlowsHooks::hook_enable(&*self.0, id, by)
    }
    fn hook_disable(&self, id: &str, by: &str) -> Result<(), String> {
        FlowsHooks::hook_disable(&*self.0, id, by)
    }
    fn hook_public(&self, id: &str, minutes: u64, by: &str) -> Result<i64, String> {
        FlowsHooks::hook_public(&*self.0, id, minutes, by)
    }
}

impl FlowsChannelPort for JobsPort {
    fn chan_allowed(&self, flow: &str) -> bool {
        FlowsChannel::chan_allowed(&*self.0, flow)
    }
    fn chan_set(&self, flow: &str, on: bool) -> Result<(), String> {
        FlowsChannel::chan_set(&*self.0, flow, on)
    }
    fn msg_policy_get(&self) -> MsgPolicy {
        FlowsChannel::msg_policy_get(&*self.0)
    }
    fn msg_policy_set(&self, p: MsgPolicy) -> Result<MsgPolicy, String> {
        FlowsChannel::msg_policy_set(&*self.0, p)
    }
    fn msg_held(&self) -> Vec<Held> {
        FlowsChannel::msg_held(&*self.0)
    }
    fn msg_watching(&self) -> Vec<MsgWatch> {
        FlowsChannel::msg_watching(&*self.0)
    }
    fn msg_ack(&self, flow: &str) -> usize {
        FlowsChannel::msg_ack(&*self.0, flow)
    }
}

impl FlowsPort for JobsPort {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    /// The method names of the job traits in a source text, as `Trait::method` with the `Port` suffix of the model crate's names dropped.
    fn methods(src: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut cur = String::new();
        for line in src.lines() {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("pub trait Flows") {
                cur = rest.split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or("").trim_end_matches("Port").to_string();
            } else if line.starts_with("}") {
                cur.clear();
            } else if let (false, Some(rest)) = (cur.is_empty(), line.strip_prefix("    fn ")) {
                let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                out.insert(format!("{cur}::{name}"));
            }
        }
        out
    }

    #[test]
    fn every_job_method_is_forwarded() {
        let api = methods(include_str!("../../../iyakki-agent-api/src/flows_jobs.rs"));
        let model = methods(include_str!("../../../iyakki-flows-model/src/port.rs"));
        let mut api = api;
        // the chat handle returns the agent API's own chat type: the tile gets its assistant pane another way
        assert!(api.remove("Meta::chat_handle"));
        // the whole-tile trait (`reg`) is the model crate's `FlowsPort`; the in-process panel hands out the engine, which cannot cross
        api.retain(|m| !m.starts_with("Jobs::") && !m.starts_with("Panel::"));
        assert!(!api.is_empty() && api.len() > 70, "the scan reads the traits: {}", api.len());
        let missing: Vec<_> = api.difference(&model).collect();
        let extra: Vec<_> = model.difference(&api).filter(|m| !m.starts_with("Jobs::") && !m.starts_with("Panel::") && !m.starts_with("FailStore::")).collect();
        assert!(missing.is_empty(), "a job method of the agent API is not in the model crate's port (add it there and in JobsPort): {missing:?}");
        assert!(extra.is_empty(), "the model crate's port has a method the agent API's job traits do not: {extra:?}");
    }
}
