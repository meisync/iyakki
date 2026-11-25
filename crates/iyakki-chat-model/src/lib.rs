//! The plain types of the chat: a message and its role, the tool call and tool description, one model turn, the `ChatBackend` trait and the
//! stand-in used when no model is configured. No network, no shell: `iyakki_shared::chat` re-exports every item under the old path and keeps the
//! OpenAI-compatible client (alpha.9 P6, D-S2-01).

pub mod cardtext;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
    /// The result of a tool call, answering an assistant message's `tool_calls`.
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// A tool invocation requested by the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// What the model is told about a tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments object.
    pub schema: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Set on assistant messages that request tools.
    pub tool_calls: Vec<ToolCall>,
    /// Set on `Role::Tool` messages: which call this answers (and the tool's name).
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    /// PNG or JPEG files the (user) message shows to a model that can look at pictures (a flow's agent step with a screenshot).
    pub images: Vec<Vec<u8>>,
}

impl Message {
    fn plain(role: Role, s: impl Into<String>) -> Self {
        Self { role, content: s.into(), tool_calls: vec![], tool_call_id: None, tool_name: None, images: vec![] }
    }
    /// A user message with pictures.
    pub fn user_with_images(s: impl Into<String>, images: Vec<Vec<u8>>) -> Self {
        Self { images, ..Self::plain(Role::User, s) }
    }
    pub fn user(s: impl Into<String>) -> Self {
        Self::plain(Role::User, s)
    }
    pub fn assistant(s: impl Into<String>) -> Self {
        Self::plain(Role::Assistant, s)
    }
    pub fn system(s: impl Into<String>) -> Self {
        Self::plain(Role::System, s)
    }
    /// An assistant turn that asks for tools (and may carry some text too).
    pub fn assistant_calls(text: impl Into<String>, calls: Vec<ToolCall>) -> Self {
        Self { tool_calls: calls, ..Self::plain(Role::Assistant, text) }
    }
    pub fn tool_result(call: &ToolCall, content: impl Into<String>) -> Self {
        Self { tool_call_id: Some(call.id.clone()), tool_name: Some(call.name.clone()), ..Self::plain(Role::Tool, content) }
    }
}

/// One model response: text and/or tool requests.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelTurn {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

pub trait ChatBackend: Send + Sync {
    /// The provider's name from the settings, for error messages in plain words (empty when not known).
    fn provider_name(&self) -> String {
        String::new()
    }

    fn complete(&self, history: &[Message]) -> Result<String, String>;

    /// Like [`complete`](ChatBackend::complete) but the model may answer with tool calls instead.
    /// The default supports no tools, so only providers that implement it can run agents.
    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        if tools.is_empty() {
            return self.complete(history).map(|text| ModelTurn { text, calls: vec![] });
        }
        Err("this model backend does not support tool calling".into())
    }

    /// Like [`complete_tools`](ChatBackend::complete_tools) but the answer is reported as it is written: `on_text` is called
    /// with each piece of visible text (and with `""` when the model is busy but has shown nothing yet, for example while it
    /// thinks or writes a tool call). Returning `false` from it asks the backend to stop early. The default does not stream:
    /// the whole turn arrives at once.
    fn complete_tools_stream(&self, history: &[Message], tools: &[ToolSpec], _on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        self.complete_tools(history, tools)
    }
}


/// Used when no model is configured, so the app explains itself instead of failing silently.
pub struct Unconfigured;

impl ChatBackend for Unconfigured {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("No chat model configured. Open Settings → Models and add a provider.".into())
    }
}

/// Keep the system prompt plus the most recent `max` turns so long chats stay within context.
pub fn trim_history(h: &[Message], max: usize) -> Vec<Message> {
    let (sys, rest): (Vec<_>, Vec<_>) = h.iter().cloned().partition(|m| m.role == Role::System);
    let skip = rest.len().saturating_sub(max);
    sys.into_iter().chain(rest.into_iter().skip(skip)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_set_the_role_and_nothing_else() {
        assert_eq!(Message::user("a").role, Role::User);
        assert_eq!(Message::assistant("a").role, Role::Assistant);
        assert_eq!(Message::system("a").role, Role::System);
        let m = Message::user_with_images("look", vec![vec![1, 2]]);
        assert_eq!((m.content.as_str(), m.images.len()), ("look", 1));
        assert!(Message::user("x").tool_calls.is_empty() && Message::user("x").images.is_empty());
    }

    #[test]
    fn tool_result_names_the_call_it_answers() {
        let c = ToolCall { id: "c1".into(), name: "ls".into(), args: serde_json::json!({}) };
        let r = Message::tool_result(&c, "ok");
        assert_eq!((r.role, r.tool_call_id.as_deref(), r.tool_name.as_deref()), (Role::Tool, Some("c1"), Some("ls")));
        let a = Message::assistant_calls("", vec![c.clone()]);
        assert_eq!(a.tool_calls, vec![c]);
    }

    #[test]
    fn role_words_are_the_wire_words() {
        let all = [(Role::System, "system"), (Role::User, "user"), (Role::Assistant, "assistant"), (Role::Tool, "tool")];
        for (r, w) in all {
            assert_eq!(r.as_str(), w);
        }
    }

    #[test]
    fn trim_keeps_the_system_prompt_and_the_newest_turns() {
        let h = vec![Message::system("s"), Message::user("1"), Message::assistant("2"), Message::user("3")];
        let t = trim_history(&h, 2);
        let words: Vec<&str> = t.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(words, ["s", "2", "3"]);
        assert_eq!(trim_history(&h, 10).len(), 4);
    }

    struct Plain;
    impl ChatBackend for Plain {
        fn complete(&self, _: &[Message]) -> Result<String, String> {
            Ok("hi".into())
        }
    }

    #[test]
    fn a_backend_without_tools_answers_plain_and_refuses_tools() {
        let spec = ToolSpec { name: "t".into(), description: String::new(), schema: Value::Null };
        assert_eq!(Plain.complete_tools(&[], &[]).unwrap().text, "hi");
        assert!(Plain.complete_tools(&[], &[spec.clone()]).is_err());
        let mut seen = Vec::new();
        let turn = Plain.complete_tools_stream(&[], &[], &mut |t| {
            seen.push(t.to_string());
            true
        });
        assert_eq!(turn.unwrap().text, "hi");
        assert!(Unconfigured.complete(&[]).unwrap_err().contains("Settings"));
    }
}
