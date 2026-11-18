//! The assistant session an app may embed (docs/development/UI-SDK.md, "Embedding the assistant"; owner decision 2026-11-16).
//!
//! An app (an SDK app, a WASM tile, a client) opens a session for the signed-in person, hands it the tools the app itself offers and the
//! context the person is looking at (a document, a form, a selection), sends messages and reads events. Everything the model decides goes
//! through the core: the core owns the model, keeps the history, asks the person to approve a tool that changes something, and writes the
//! audit line. The app never sees a key, a provider or another tool: its tools are the only ones the session has.
//!
//! An app tool runs in the app (it changes the app's own data). The round trip: the model asks for `app_<app>_<tool>`; if the app declared
//! the tool as [`ToolApproval::Ask`] the core emits [`AssistEvent::Approval`] and waits for [`Assistant::decide`]; then it emits
//! [`AssistEvent::ToolCall`], the app runs the tool and answers with [`Assistant::answer`]; the model sees the answer. A call the person
//! denied, or one the app never answers, is told to the model as a failure.
//!
//! The limits below are the core's: a request that breaks one is refused with a sentence, never cut silently.

use serde::{Deserialize, Serialize};

/// The version of this facet (the types and the trait below). Additive changes only.
pub const ASSISTANT_FACET_VERSION: u32 = 1;

/// How many tools one session may carry, and the sizes the core accepts.
pub const MAX_TOOLS: usize = 16;
pub const MAX_TOOL_NAME: usize = 40;
pub const MAX_TOOL_DESCRIPTION: usize = 400;
pub const MAX_SCHEMA: usize = 4096;
pub const MAX_CONTEXT_ITEMS: usize = 8;
pub const MAX_CONTEXT_ITEM: usize = 16 * 1024;
pub const MAX_CONTEXT_TOTAL: usize = 48 * 1024;
pub const MAX_MESSAGE: usize = 8 * 1024;
pub const MAX_APP_ID: usize = 40;
pub const MAX_CONVERSATION: usize = 64;

/// Whether the person is asked before the app's tool runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolApproval {
    /// Reads only: the call is shown in the conversation and runs.
    Safe,
    /// Changes something: a card with the summary is shown and the call waits for the person.
    Ask,
}

/// A tool the app offers to its own assistant. `schema` is a JSON Schema object (as text) for the arguments.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppToolSpec {
    /// Lower-case letters, digits and `_`, starting with a letter. The model sees it as `app_<app>_<name>`; the app is told the plain name.
    pub name: String,
    pub description: String,
    pub schema: String,
    pub approval: ToolApproval,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    Document,
    Form,
    Selection,
    Other,
}

impl ContextKind {
    pub fn word(self) -> &'static str {
        match self {
            ContextKind::Document => "document",
            ContextKind::Form => "form",
            ContextKind::Selection => "selection",
            ContextKind::Other => "data",
        }
    }
}

/// Something the person is looking at in the app. The model reads it as untrusted data, never as instructions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextItem {
    pub kind: ContextKind,
    pub name: String,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSpec {
    /// The app's own name for the conversation: opening the same name again gives the same history back.
    pub conversation: String,
    pub title: String,
    pub tools: Vec<AppToolSpec>,
    pub context: Vec<ContextItem>,
    /// A model id from [`Assistant::models`] and a role id from [`Assistant::roles`]; none: the person's choice in Settings.
    pub model: Option<String>,
    pub role: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    /// The conversation existed already: its history is there.
    pub resumed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

/// What a session tells the app, in order. [`Assistant::poll`] hands each event out once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistEvent {
    /// The answer so far, while the model is still writing it (replaces the earlier partial).
    Partial { text: String },
    /// A line from the assistant's safety net or the core (a retry, a limit).
    Note { text: String },
    /// The model asked for a tool (shown in the conversation; `tool` is the app's plain name).
    ToolRequest { call: String, tool: String, args: String },
    /// A card: the person must decide before the call runs.
    Approval { call: String, tool: String, summary: String, detail: String },
    /// How a call was decided: `by` is `auto` (a safe tool), `user` or `system` (timeout, denied by policy).
    Decision { call: String, tool: String, approved: bool, by: String },
    /// Run this app tool now and answer with [`Assistant::answer`].
    ToolCall { call: String, tool: String, args: String },
    ToolResult { call: String, tool: String, ok: bool, text: String },
    Final { text: String },
    Failed { error: String },
    Stopped { reason: String },
}

impl AssistEvent {
    /// The turn is over: nothing more comes until the next message.
    pub fn ends_turn(&self) -> bool {
        matches!(self, AssistEvent::Final { .. } | AssistEvent::Failed { .. } | AssistEvent::Stopped { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRole {
    User,
    Assistant,
    /// A tool the app ran for the assistant (the result text).
    Tool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub role: HistoryRole,
    pub text: String,
}

/// The reason a request is refused, as a sentence for the person; used by every implementation so the words are the same.
pub fn check_spec(app: &str, spec: &SessionSpec) -> Result<(), String> {
    check_app(app)?;
    if spec.conversation.is_empty() || spec.conversation.len() > MAX_CONVERSATION || spec.conversation.chars().any(char::is_control) {
        return Err(format!("the conversation name must be 1 to {MAX_CONVERSATION} characters without control characters"));
    }
    if spec.tools.len() > MAX_TOOLS {
        return Err(format!("an app may offer at most {MAX_TOOLS} tools to its assistant"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for t in &spec.tools {
        check_tool(t)?;
        if !seen.insert(t.name.as_str()) {
            return Err(format!("the tool `{}` is offered twice", t.name));
        }
    }
    check_context(&spec.context)
}

pub fn check_app(app: &str) -> Result<(), String> {
    let ok = !app.is_empty() && app.len() <= MAX_APP_ID && app.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && app.starts_with(|c: char| c.is_ascii_lowercase());
    if ok { Ok(()) } else { Err("the app name must be lower-case letters, digits and `-`, starting with a letter".into()) }
}

pub fn check_tool(t: &AppToolSpec) -> Result<(), String> {
    let name_ok = !t.name.is_empty() && t.name.len() <= MAX_TOOL_NAME && t.name.starts_with(|c: char| c.is_ascii_lowercase()) && t.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !name_ok {
        return Err(format!("the tool name `{}` must be lower-case letters, digits and `_`, starting with a letter, at most {MAX_TOOL_NAME} long", t.name.chars().take(MAX_TOOL_NAME).collect::<String>()));
    }
    if t.description.is_empty() || t.description.len() > MAX_TOOL_DESCRIPTION {
        return Err(format!("the tool `{}` needs a description of at most {MAX_TOOL_DESCRIPTION} characters", t.name));
    }
    if t.schema.len() > MAX_SCHEMA {
        return Err(format!("the schema of `{}` is longer than {MAX_SCHEMA} characters", t.name));
    }
    match serde_json::from_str::<serde_json::Value>(&t.schema) {
        Ok(v) if v.is_object() => Ok(()),
        _ => Err(format!("the schema of `{}` is not a JSON object", t.name)),
    }
}

pub fn check_context(items: &[ContextItem]) -> Result<(), String> {
    if items.len() > MAX_CONTEXT_ITEMS {
        return Err(format!("at most {MAX_CONTEXT_ITEMS} context items may be attached"));
    }
    let mut total = 0;
    for c in items {
        if c.name.is_empty() || c.name.len() > 80 || c.name.chars().any(char::is_control) {
            return Err("a context item needs a name of 1 to 80 characters".into());
        }
        if c.text.len() > MAX_CONTEXT_ITEM {
            return Err(format!("the context item `{}` is longer than {} KB", c.name, MAX_CONTEXT_ITEM / 1024));
        }
        total += c.text.len();
    }
    if total > MAX_CONTEXT_TOTAL {
        return Err(format!("the attached context is longer than {} KB together", MAX_CONTEXT_TOTAL / 1024));
    }
    Ok(())
}

pub fn check_message(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("the message is empty".into());
    }
    if text.len() > MAX_MESSAGE {
        return Err(format!("the message is longer than {} KB", MAX_MESSAGE / 1024));
    }
    Ok(())
}

/// The name the model sees for an app's tool.
pub fn model_tool_name(app: &str, tool: &str) -> String {
    format!("app_{}_{}", app.replace('-', "_"), tool)
}

/// The assistant of the core, as an app embeds it. Every call names the signed-in person; a session belongs to the person who opened
/// it and to the app that named itself at `open`, and another person's session id is refused like an unknown one.
pub trait Assistant: Send + Sync {
    /// The models the person may choose between for an embedded assistant (the first is the default). Empty: this core has no assistant.
    fn models(&self, _user: &str) -> Vec<Choice> {
        Vec::new()
    }
    /// The roles (how the assistant behaves) the person may choose between.
    fn roles(&self, _user: &str) -> Vec<Choice> {
        Vec::new()
    }
    /// Open (or reopen) the conversation `spec.conversation` of `app` for `user`. The tools and the context replace those of an earlier open.
    fn open(&self, _user: &str, _app: &str, _spec: &SessionSpec) -> Result<SessionInfo, String> {
        Err("this core has no embedded assistant".into())
    }
    /// Replace the attached context (the person moved to another document or selected something else).
    fn set_context(&self, _user: &str, _session: &str, _context: &[ContextItem]) -> Result<(), String> {
        Err("this core has no embedded assistant".into())
    }
    /// Change the model and the role of the session (`None` keeps the current one).
    fn choose(&self, _user: &str, _session: &str, _model: Option<&str>, _role: Option<&str>) -> Result<(), String> {
        Err("this core has no embedded assistant".into())
    }
    /// A message from the person. Refused while the previous turn is still running.
    fn send(&self, _user: &str, _session: &str, _text: &str) -> Result<(), String> {
        Err("this core has no embedded assistant".into())
    }
    /// The events since the last call, in order.
    fn poll(&self, _user: &str, _session: &str) -> Vec<AssistEvent> {
        Vec::new()
    }
    /// The person's answer to an [`AssistEvent::Approval`] card.
    fn decide(&self, _user: &str, _session: &str, _call: &str, _approve: bool) -> Result<(), String> {
        Err("this core has no embedded assistant".into())
    }
    /// The app's answer to an [`AssistEvent::ToolCall`].
    fn answer(&self, _user: &str, _session: &str, _call: &str, _ok: bool, _text: &str) -> Result<(), String> {
        Err("this core has no embedded assistant".into())
    }
    /// Stop the turn that is running (the model is cancelled, a waiting approval is denied).
    fn cancel(&self, _user: &str, _session: &str) -> Result<(), String> {
        Err("this core has no embedded assistant".into())
    }
    fn history(&self, _user: &str, _session: &str) -> Vec<HistoryEntry> {
        Vec::new()
    }
    /// Close the session (the history of the conversation is kept for the next open).
    fn close(&self, _user: &str, _session: &str) {}
}

/// The assistant of a core that has none: every call is refused with a sentence, the lists are empty.
pub struct NoAssistant;

impl Assistant for NoAssistant {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> AppToolSpec {
        AppToolSpec { name: name.into(), description: "adds a note".into(), schema: "{\"type\":\"object\"}".into(), approval: ToolApproval::Ask }
    }

    fn spec(tools: Vec<AppToolSpec>) -> SessionSpec {
        SessionSpec { conversation: "main".into(), title: "T".into(), tools, ..Default::default() }
    }

    #[test]
    fn the_limits_refuse_with_a_sentence() {
        assert!(check_spec("sample", &spec(vec![tool("add_note")])).is_ok());
        assert!(check_spec("Sample", &spec(vec![])).unwrap_err().contains("lower-case"));
        assert!(check_spec("sample", &spec(vec![tool("Add")])).unwrap_err().contains("tool name"));
        assert!(check_spec("sample", &spec(vec![tool("a"), tool("a")])).unwrap_err().contains("twice"));
        let mut bad = tool("x");
        bad.schema = "[1]".into();
        assert!(check_spec("sample", &spec(vec![bad])).unwrap_err().contains("not a JSON object"));
        let many: Vec<_> = (0..=MAX_TOOLS).map(|i| tool(&format!("t{i}"))).collect();
        assert!(check_spec("sample", &spec(many)).unwrap_err().contains("at most"));
        let mut s = spec(vec![]);
        s.context = vec![ContextItem { kind: ContextKind::Document, name: "doc".into(), text: "x".repeat(MAX_CONTEXT_ITEM + 1) }];
        assert!(check_spec("sample", &s).unwrap_err().contains("longer"));
        assert!(check_message("  ").is_err() && check_message("hi").is_ok());
        assert_eq!(model_tool_name("my-app", "add_note"), "app_my_app_add_note");
    }

    #[test]
    fn a_core_with_no_assistant_refuses_and_lists_nothing() {
        let n = NoAssistant;
        assert!(n.models("u").is_empty() && n.roles("u").is_empty());
        assert!(n.open("u", "sample", &spec(vec![])).unwrap_err().contains("no embedded assistant"));
        assert!(n.send("u", "s", "hi").is_err() && n.poll("u", "s").is_empty());
    }

    #[test]
    fn events_have_a_stable_wire_shape() {
        let e = AssistEvent::Approval { call: "c1".into(), tool: "add_note".into(), summary: "Add a note".into(), detail: String::new() };
        let j = serde_json::to_string(&e).unwrap();
        assert_eq!(j, "{\"type\":\"approval\",\"call\":\"c1\",\"tool\":\"add_note\",\"summary\":\"Add a note\",\"detail\":\"\"}");
        assert_eq!(serde_json::from_str::<AssistEvent>(&j).unwrap(), e);
        assert!(AssistEvent::Final { text: String::new() }.ends_turn() && !AssistEvent::Partial { text: String::new() }.ends_turn());
    }
}
