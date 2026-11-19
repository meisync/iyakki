//! Types moved out of `iyakki_agent::tokenbudget` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// What one request carries, in estimated tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Breakdown {
    /// The system message(s), including the rules and the project state added to it.
    pub system: u64,
    /// The definitions of the offered tools (name, description, parameter schema).
    pub tools: u64,
    /// The conversation: the person's messages and the assistant's text and tool requests.
    pub history: u64,
    /// The results of tool calls that sit in the conversation.
    pub outputs: u64,
    /// How many tools the model is offered in this request (the routing meta-tool counts as one).
    pub offered: u32,
    /// How many tools the account has right now.
    pub total: u32,
}

impl Breakdown {
    pub fn sum(&self) -> u64 {
        self.system + self.tools + self.history + self.outputs
    }

    /// The log line and the footer detail.
    pub fn line(&self) -> String {
        format!("tokens system={} tools={} history={} outputs={} tools_offered={} of {}", self.system, self.tools, self.history, self.outputs, self.offered, self.total)
    }
}
