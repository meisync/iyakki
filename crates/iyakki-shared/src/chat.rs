//! CHAT: a small client for OpenAI-compatible `/v1/chat/completions` endpoints (OpenAI, Ollama,
//! vLLM, llama.cpp server, …). Requests go through the egress proxy, never straight to the network.

use std::sync::Arc;

use iyakki_wasm::NetBackend;
use serde_json::{json, Value};

pub use iyakki_chat_model::{trim_history, ChatBackend, Message, ModelTurn, Role, ToolCall, ToolSpec, Unconfigured};

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
