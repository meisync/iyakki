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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_adds_the_four_token_buckets_and_ignores_the_tool_counts() {
        let b = Breakdown { system: 1, tools: 2, history: 4, outputs: 8, offered: 99, total: 99 };
        assert_eq!(b.sum(), 15);
    }

    #[test]
    fn sum_of_an_empty_breakdown_is_zero() {
        assert_eq!(Breakdown::default().sum(), 0);
    }

    #[test]
    fn line_lists_each_bucket_then_the_offered_and_total_tool_counts() {
        let b = Breakdown { system: 10, tools: 20, history: 30, outputs: 40, offered: 3, total: 7 };
        assert_eq!(b.line(), "tokens system=10 tools=20 history=30 outputs=40 tools_offered=3 of 7");
    }

    #[test]
    fn line_of_an_empty_breakdown_shows_only_zeros() {
        assert_eq!(Breakdown::default().line(), "tokens system=0 tools=0 history=0 outputs=0 tools_offered=0 of 0");
    }
}
