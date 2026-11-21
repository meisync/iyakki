//! The FLOWS tile's views of the core's assistant book and outside-message policy (F-SPLIT-03, D-S5-01): the same calls the tile always
//! made on `AssistStore` and `PolicyBook`, answered by the jobs of `FlowsJobs` (so the same code runs against the real environment and
//! against the remote objects of the UI program, and no file of the core is opened by the tile).

use iyakki_agent_api::FlowsJobs;
use iyakki_flow::assist_profile::AgentProfile;
use iyakki_flow::assist_store::{Book, Global, Status, Suggestion, Watch};
use iyakki_shared::flows_channel_policy::{Held, Policy, Watch as MsgWatch};

/// The assistant's book of one account.
pub(super) struct AssistBook<'a>(pub &'a dyn FlowsJobs);

pub(super) fn assist(env: &dyn FlowsJobs) -> AssistBook<'_> {
    AssistBook(env)
}

impl AssistBook<'_> {
    pub fn global(&self) -> Global {
        self.0.assist_global()
    }
    pub fn set_enabled(&self, on: bool) -> Result<(), String> {
        self.0.assist_set_enabled(on)
    }
    pub fn set_global_cap(&self, per_day: u32) -> Result<(), String> {
        self.0.assist_set_global_cap(per_day)
    }
    pub fn set_matrix(&self, on: bool, room: &str) -> Result<(), String> {
        self.0.assist_set_matrix(on, room)
    }
    pub fn set_profile(&self, p: AgentProfile) -> Result<AgentProfile, String> {
        self.0.assist_set_profile(p)
    }
    pub fn book(&self, flow: &str) -> Book {
        self.0.assist_book(flow)
    }
    pub fn open_list(&self, flow: &str) -> Vec<Suggestion> {
        self.0.assist_open_list(flow)
    }
    pub fn now(&self) -> i64 {
        self.0.assist_now()
    }
    pub fn watch(&self, flow: &str) -> Option<Watch> {
        self.0.assist_watch(flow)
    }
    pub fn set_watch(&self, flow: &str, w: Option<Watch>) -> Result<(), String> {
        self.0.assist_set_watch(flow, w)
    }
    pub fn set_status(&self, flow: &str, sid: &str, st: Status) -> Result<Suggestion, String> {
        self.0.assist_set_status(flow, sid, st)
    }
    pub fn paused(&self, flow: &str) -> bool {
        self.0.assist_paused(flow)
    }
    pub fn set_paused(&self, flow: &str, on: bool) -> Result<(), String> {
        self.0.assist_set_paused(flow, on)
    }
    pub fn readout(&self, flow: &str, now_ms: i64, limit: usize) -> serde_json::Value {
        self.0.assist_readout(flow, now_ms, limit)
    }
}

/// The outside-message policy of one account.
pub(super) struct MsgBook<'a>(pub &'a dyn FlowsJobs);

pub(super) fn msg(env: &dyn FlowsJobs) -> MsgBook<'_> {
    MsgBook(env)
}

impl MsgBook<'_> {
    pub fn policy(&self) -> Policy {
        self.0.msg_policy_get()
    }
    pub fn set_policy(&self, p: Policy) -> Result<Policy, String> {
        self.0.msg_policy_set(p)
    }
    pub fn held(&self) -> Vec<Held> {
        self.0.msg_held()
    }
    pub fn watching(&self) -> Vec<MsgWatch> {
        self.0.msg_watching()
    }
    pub fn ack(&self, flow: &str) -> usize {
        self.0.msg_ack(flow)
    }
}
