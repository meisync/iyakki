//! CHAT: a small client for OpenAI-compatible `/v1/chat/completions` endpoints (OpenAI, Ollama,
//! vLLM, llama.cpp server, …). Requests go through the egress proxy, never straight to the network.

use std::sync::Arc;

use iyakki_wasm::NetBackend;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
#[derive(Clone, Debug, PartialEq)]
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
}

impl Message {
    fn plain(role: Role, s: impl Into<String>) -> Self {
        Self { role, content: s.into(), tool_calls: vec![], tool_call_id: None, tool_name: None }
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
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ModelTurn {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

pub trait ChatBackend: Send + Sync {
    fn complete(&self, history: &[Message]) -> Result<String, String>;

    /// Like [`complete`](ChatBackend::complete) but the model may answer with tool calls instead.
    /// The default supports no tools, so only providers that implement it can run agents.
    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        if tools.is_empty() {
            return self.complete(history).map(|text| ModelTurn { text, calls: vec![] });
        }
        Err("this model backend does not support tool calling".into())
    }
}

/// Used when no model is configured, so the app explains itself instead of failing silently.
pub struct Unconfigured;

impl ChatBackend for Unconfigured {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("No chat model configured. Open Settings → Models and add a provider.".into())
    }
}

pub struct OpenAiCompat {
    net: Arc<dyn NetBackend>,
    url: String,
    model: String,
}

impl OpenAiCompat {
    pub fn new(net: Arc<dyn NetBackend>, url: &str, model: &str) -> Self {
        Self { net, url: url.into(), model: model.into() }
    }
}

/// Keep the system prompt plus the most recent `max` turns so long chats stay within context.
pub fn trim_history(h: &[Message], max: usize) -> Vec<Message> {
    let (sys, rest): (Vec<_>, Vec<_>) = h.iter().cloned().partition(|m| m.role == Role::System);
    let skip = rest.len().saturating_sub(max);
    sys.into_iter().chain(rest.into_iter().skip(skip)).collect()
}

impl ChatBackend for OpenAiCompat {
    fn complete(&self, history: &[Message]) -> Result<String, String> {
        let msgs: Vec<Value> = trim_history(history, 24).iter().map(|m| json!({"role": m.role.as_str(), "content": m.content})).collect();
        let body = json!({"model": self.model, "messages": msgs, "stream": false}).to_string();
        let resp = self
            .net
            .fetch("POST", &self.url, body.as_bytes())
            .map_err(|e| format!("request failed or blocked (code {e})"))?;
        let v: Value = serde_json::from_slice(&resp).map_err(|_| "model returned something that is not JSON".to_string())?;
        if let Some(err) = v.pointer("/error/message").and_then(Value::as_str) {
            return Err(format!("model error: {err}"));
        }
        v.pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "unexpected response shape (no choices[0].message.content)".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake(Mutex<Vec<String>>, Result<Vec<u8>, i32>);
    impl NetBackend for Fake {
        fn fetch(&self, m: &str, u: &str, b: &[u8]) -> Result<Vec<u8>, i32> {
            self.0.lock().unwrap().push(format!("{m} {u} {}", String::from_utf8_lossy(b)));
            self.1.clone()
        }
    }

    #[test]
    fn builds_request_and_parses_reply() {
        let f = Arc::new(Fake(Default::default(), Ok(br#"{"choices":[{"message":{"role":"assistant","content":"hi there"}}]}"#.to_vec())));
        let c = OpenAiCompat::new(f.clone(), "https://h/v1/chat/completions", "m1");
        let out = c.complete(&[Message::system("be nice"), Message::user("hello")]).unwrap();
        assert_eq!(out, "hi there");
        let sent = f.0.lock().unwrap()[0].clone();
        assert!(sent.starts_with("POST https://h/v1/chat/completions "));
        let body: Value = serde_json::from_str(sent.split_once(' ').unwrap().1.split_once(' ').unwrap().1).unwrap();
        assert_eq!(body["model"], "m1");
        assert_eq!(body["messages"][1]["content"], "hello");
    }

    #[test]
    fn surfaces_errors() {
        let blocked = OpenAiCompat::new(Arc::new(Fake(Default::default(), Err(-2))), "https://h/", "m");
        assert!(blocked.complete(&[Message::user("x")]).unwrap_err().contains("blocked"));
        let err = OpenAiCompat::new(Arc::new(Fake(Default::default(), Ok(br#"{"error":{"message":"bad key"}}"#.to_vec()))), "https://h/", "m");
        assert_eq!(err.complete(&[Message::user("x")]).unwrap_err(), "model error: bad key");
        let junk = OpenAiCompat::new(Arc::new(Fake(Default::default(), Ok(b"<html>".to_vec()))), "https://h/", "m");
        assert!(junk.complete(&[Message::user("x")]).is_err());
        assert!(Unconfigured.complete(&[]).unwrap_err().contains("Settings"));
    }

    #[test]
    fn history_is_trimmed_keeping_system() {
        let mut h = vec![Message::system("s")];
        for i in 0..10 {
            h.push(Message::user(format!("u{i}")));
        }
        let t = trim_history(&h, 3);
        assert_eq!(t.len(), 4);
        assert_eq!(t[0].role, Role::System);
        assert_eq!(t[3].content, "u9");
    }
}
