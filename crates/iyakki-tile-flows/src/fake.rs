//! A fake `FlowsPort` for the tile's own tests: flows kept in memory, no engine, no files, no threads, no clock. Calls that change something are
#![allow(unused_variables)]
//! recorded (`audit`, `saved`, `started`) so a test can say what the tile asked for. Everything the screens read but a test does not set is empty.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use iyakki_flow::assist_store::{Book, Global, Status, Suggestion};
use iyakki_flow::engine::{ApplyReport, ApplyScope, CardDecision, Counts, NodeTest, NodeUpdateInfo, StartOutcome, StartRequest};
use iyakki_flow::host::BrowserSessionInfo;
use iyakki_flow::kinds::Issue;
use iyakki_flow::model::{Flow, Run, RunSummary};
use iyakki_flow::store::{FlowMeta, FlowSummary, VersionEntry};
use iyakki_flows_model::channel::{Held, Policy as MsgPolicy, Watch as MsgWatch};
use iyakki_flows_model::hookview::{Created, HookOpts, HookView};
use iyakki_flows_model::port::*;
use iyakki_flows_model::port::policy::Policy;

#[derive(Default)]
pub struct State {
    pub flows: BTreeMap<String, Flow>,
    pub runs: Vec<RunSummary>,
    pub audit: Vec<(String, String)>,
    pub saved: Vec<String>,
    pub started: Vec<String>,
    pub allowed: Vec<String>,
    pub last: Option<(String, String)>,
    pub policy: MsgPolicy,
    pub assist_on: bool,
    pub rev: u64,
    pub stamp: u128,
}

#[derive(Default)]
pub struct FakePort(pub Mutex<State>);

impl FakePort {
    pub fn st(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn with_flow(self: Arc<Self>, id: &str, name: &str) -> Arc<Self> {
        self.st().flows.insert(id.into(), Flow::new(id, name));
        self
    }
}

impl FlowsMetaPort for FakePort {
    fn user(&self) -> String {
        "root".into()
    }
    fn policy(&self) -> Policy {
        iyakki_flows_model::port::policy::Role::Admin.template()
    }
    fn data(&self) -> PathBuf {
        std::path::PathBuf::from("/fake/flows/root")
    }
    fn dir(&self) -> PathBuf {
        std::path::PathBuf::from("/fake/home/Flows")
    }
    fn svc_dir(&self) -> PathBuf {
        std::path::PathBuf::from("/fake/svc")
    }
    fn who(&self) -> String {
        "user:root".into()
    }
    fn audit(&self, action: &str, detail: &str) {
        self.st().audit.push((action.to_string(), detail.to_string()));
    }
    fn flow_names(&self) -> Vec<(String, String)> {
        self.st().flows.values().map(|f| (f.id.clone(), f.name.clone())).collect()
    }
    fn set_last_flow(&self, flow: &str, name: &str) {
        self.st().last = Some((flow.to_string(), name.to_string()));
    }
    fn vision_line(&self) -> String {
        "the vision model is not set".into()
    }
    fn refresh_model_lists(&self) -> String {
        "nothing to refresh".into()
    }
}

impl FlowsDraftsPort for FakePort {
    fn summaries(&self) -> Vec<FlowSummary> {
        self.st().flows.values().map(|f| FlowSummary { id: f.id.clone(), name: f.name.clone(), description: f.description.clone(), published: 0, nodes: f.nodes.len(), triggers: f.triggers.len(), draft_changed: false, agent_changed: false, production: false }).collect()
    }
    fn issues(&self, f: &Flow) -> Vec<Issue> {
        vec![]
    }
    fn get_draft(&self, id: &str) -> Option<Flow> {
        self.st().flows.get(id).cloned()
    }
    fn published(&self, id: &str) -> Option<Flow> {
        None
    }
    fn versions(&self, id: &str) -> Vec<VersionEntry> {
        vec![]
    }
    fn get_version(&self, id: &str, version: u32) -> Option<Flow> {
        None
    }
    fn meta_of(&self, id: &str) -> FlowMeta {
        FlowMeta::default()
    }
    fn draft_stamp(&self, id: &str) -> (u64, u128) {
        let s = self.st();
        (s.flows.get(id).map_or(0, |f| f.nodes.len() as u64), s.stamp)
    }
    fn rollback(&self, id: &str, to: u32, by: &str) -> Result<u32, String> {
        Err("no versions in the fake".into())
    }
    fn publish(&self, id: &str, by: &str, label: &str, changelog: &str, approved_by: Option<String>) -> Result<u32, String> {
        Err("publishing is not in the fake".into())
    }
    fn save_edit(&self, flow: &Flow, by: &str, summary: &str, node: &str) -> Result<(), String> {
        let mut s = self.st();
        s.stamp += 1;
        s.saved.push(format!("{} by {by}: {summary}", flow.id));
        s.flows.insert(flow.id.clone(), flow.clone());
        Ok(())
    }
    fn create_draft(&self, f: Flow, by: &str) -> Result<Flow, String> {
        let mut s = self.st();
        if s.flows.contains_key(&f.id) {
            return Err(format!("`{}` exists", f.id));
        }
        s.stamp += 1;
        s.flows.insert(f.id.clone(), f.clone());
        Ok(f)
    }
    fn import_json(&self, text: &str, by: &str) -> Result<Flow, String> {
        let f = Flow::from_json(text)?;
        self.create_draft(f, by)
    }
    fn export_json(&self, id: &str) -> Result<String, String> {
        self.st().flows.get(id).map(|f| f.to_json()).ok_or_else(|| "no such flow".to_string())
    }
    fn from_template(&self, template: &str, name: &str, by: &str) -> Result<Flow, String> {
        let t = iyakki_flow::templates::get(template).ok_or("no such template")?;
        let id = name.to_lowercase().replace(' ', "-");
        self.create_draft(iyakki_flow::templates::instantiate(&t, &id, name), by)
    }
    fn allow_program(&self, name: &str) -> Result<(), String> {
        self.st().allowed.push(name.to_string());
        Ok(())
    }
    fn drop_version(&self, id: &str, version: u32) -> Result<(), String> {
        Err("no versions in the fake".into())
    }
    fn delete_flow(&self, id: &str) -> Result<String, String> {
        let mut s = self.st();
        s.flows.remove(id).map(|f| f.name).ok_or_else(|| "no such flow".to_string())
    }
}

impl FlowsRunsPort for FakePort {
    fn start_run(&self, req: StartRequest) -> Result<StartOutcome, String> {
        let mut s = self.st();
        let id = format!("run-{}", s.runs.len() + 1);
        s.started.push(req.flow_id.clone());
        s.rev += 1;
        Ok(StartOutcome { run_id: id, existing: false })
    }
    fn get_run(&self, id: &str) -> Option<Run> {
        None
    }
    fn run_summaries(&self) -> Vec<RunSummary> {
        self.st().runs.clone()
    }
    fn run_counts(&self) -> Counts {
        Counts::default()
    }
    fn revision(&self) -> u64 {
        self.st().rev
    }
    fn ticking(&self) -> bool {
        true
    }
    fn next_runs(&self, flow: &Flow, n: usize) -> Vec<(String, i64)> {
        vec![]
    }
    fn decide(&self, run_id: &str, node: &str, approved: bool, by: &str, note: &str) -> Result<(), String> {
        Ok(())
    }
    fn card_decide(&self, run_id: &str, card: &str, d: CardDecision) -> Result<(), String> {
        Ok(())
    }
    fn cancel(&self, run_id: &str) -> Result<(), String> {
        Ok(())
    }
    fn retry_node(&self, run_id: &str, node: &str) -> Result<(), String> {
        Ok(())
    }
    fn skip_node(&self, run_id: &str, node: &str, by: &str) -> Result<(), String> {
        Ok(())
    }
    fn rerun_from(&self, run_id: &str, node: &str) -> Result<(), String> {
        Ok(())
    }
    fn node_updates(&self, flow_id: &str) -> Vec<NodeUpdateInfo> {
        vec![]
    }
    fn apply_node_update(&self, flow_id: &str, node: &str, scope: ApplyScope, by: &str, allow_incompatible: bool) -> Result<ApplyReport, String> {
        Ok(ApplyReport::default())
    }
    fn test_node(&self, flow: &Flow, node_id: &str, sample_inputs: serde_json::Value, input: serde_json::Value) -> Result<NodeTest, String> {
        Ok(NodeTest { ok: true, output: serde_json::json!({}), port: "out".into(), error: String::new(), logs: vec![], waiting: false })
    }
    fn report_run(&self, run_id: &str, flow_id: &str) {
    }
    fn withdraw_permissions(&self, run_id: &str) -> usize {
        0
    }
    fn browser_session(&self, run_id: &str) -> Option<BrowserSessionInfo> {
        None
    }
}

impl FlowsAssistPort for FakePort {
    fn assist_global(&self) -> Global {
        Global::default()
    }
    fn assist_set_enabled(&self, on: bool) -> Result<(), String> {
        self.st().assist_on = on;
        Ok(())
    }
    fn assist_set_global_cap(&self, per_day: u32) -> Result<(), String> {
        Ok(())
    }
    fn assist_set_matrix(&self, on: bool, room: &str) -> Result<(), String> {
        Ok(())
    }
    fn assist_set_profile(&self, p: iyakki_flow::assist_profile::AgentProfile) -> Result<iyakki_flow::assist_profile::AgentProfile, String> {
        Ok(p)
    }
    fn assist_book(&self, flow: &str) -> Book {
        Book::default()
    }
    fn assist_open_list(&self, flow: &str) -> Vec<Suggestion> {
        vec![]
    }
    fn assist_now(&self) -> i64 {
        1_000_000
    }
    fn assist_watch(&self, flow: &str) -> Option<iyakki_flow::assist_store::Watch> {
        None
    }
    fn assist_set_watch(&self, flow: &str, w: Option<iyakki_flow::assist_store::Watch>) -> Result<(), String> {
        Ok(())
    }
    fn assist_set_status(&self, flow: &str, sid: &str, st: Status) -> Result<Suggestion, String> {
        Err("no suggestions in the fake".into())
    }
    fn assist_paused(&self, flow: &str) -> bool {
        false
    }
    fn assist_set_paused(&self, flow: &str, on: bool) -> Result<(), String> {
        Ok(())
    }
    fn assist_readout(&self, flow: &str, now_ms: i64, limit: usize) -> serde_json::Value {
        serde_json::json!({})
    }
    fn assist_adopt(&self, flow: &str, sid: &str) -> Result<Suggestion, String> {
        Err("no suggestions in the fake".into())
    }
    fn assist_reject(&self, flow: &str, sid: &str, by: &str) -> Result<Suggestion, String> {
        Err("no suggestions in the fake".into())
    }
    fn trial_whole(&self, flow: &str) -> Result<iyakki_flow::assist_run::WholeTrial, String> {
        Ok(iyakki_flow::assist_run::WholeTrial::default())
    }
    fn assist_after_publish(&self, flow: &str, version: u32) {
    }
}

impl FlowsHooksPort for FakePort {
    fn hook_views(&self, flow: Option<&str>) -> Vec<HookView> {
        vec![]
    }
    fn hook_create(&self, flow: &str, o: &HookOpts, by: &str) -> Result<Created, String> {
        Err("no webhooks in the fake".into())
    }
    fn hook_regenerate(&self, id: &str, by: &str) -> Result<Created, String> {
        Err("no webhooks in the fake".into())
    }
    fn hook_enable(&self, id: &str, by: &str) -> Result<(), String> {
        Ok(())
    }
    fn hook_disable(&self, id: &str, by: &str) -> Result<(), String> {
        Ok(())
    }
    fn hook_public(&self, id: &str, minutes: u64, by: &str) -> Result<i64, String> {
        Ok(0)
    }
}

impl FlowsChannelPort for FakePort {
    fn chan_allowed(&self, flow: &str) -> bool {
        true
    }
    fn chan_set(&self, flow: &str, on: bool) -> Result<(), String> {
        Ok(())
    }
    fn msg_policy_get(&self) -> MsgPolicy {
        self.st().policy.clone()
    }
    fn msg_policy_set(&self, p: MsgPolicy) -> Result<MsgPolicy, String> {
        let p = p.cleaned()?;
        self.st().policy = p.clone();
        Ok(p)
    }
    fn msg_held(&self) -> Vec<Held> {
        vec![]
    }
    fn msg_watching(&self) -> Vec<MsgWatch> {
        vec![]
    }
    fn msg_ack(&self, flow: &str) -> usize {
        0
    }
}

impl FlowsPort for FakePort {}
