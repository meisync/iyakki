//! The assistant port: what an SDK app is handed so that it can embed the IYAKKI assistant (see [`crate::chatview::ChatView`]). Like the
//! storage port it carries no pixels and opens no socket: the host gives the app a port, and the port is the core's. One port is one
//! session (the person and the app are bound by the host), so no call names a session.
//!
//! The host-side meaning, in one paragraph: the core owns the model, the history and the person's approvals. The app declares its own
//! tools (a name, a description, a JSON Schema, and whether the person is asked first); the model can call only those. An app tool runs in
//! the app: the port hands out [`ChatEvent::ToolCall`], the app does the work on its own data and answers with [`AssistantPort::answer`].
//! A tool that changes something is `ask: true`: the core shows a card and waits for the person ([`ChatEvent::Approval`]), and writes the
//! audit line. [`ScriptedAssistant`] is the in-memory port for tests and the browser preview: a scripted model that follows the same rules.

use std::collections::{BTreeMap, VecDeque};

/// The most characters a scripted fake keeps of a message (the real core has its own limits and refuses with a sentence).
pub const MAX_TEXT: usize = 8 * 1024;

/// A tool the app offers to its own assistant. `schema` is JSON Schema text for the arguments object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: String,
    /// The person is asked before the tool runs (it changes something).
    pub ask: bool,
}

impl ToolSpec {
    pub fn new(name: &str, description: &str, schema: &str, ask: bool) -> ToolSpec {
        ToolSpec { name: name.into(), description: description.into(), schema: schema.into(), ask }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {
    Document,
    Form,
    Selection,
    Other,
}

/// Something the person is looking at in the app; the model reads it as untrusted data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextItem {
    pub kind: ContextKind,
    pub name: String,
    pub text: String,
}

impl ContextItem {
    pub fn new(kind: ContextKind, name: &str, text: &str) -> ContextItem {
        ContextItem { kind, name: name.into(), text: text.into() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionConfig {
    /// The app's own name for the conversation: opening the same name again gives the history back.
    pub conversation: String,
    pub title: String,
    pub tools: Vec<ToolSpec>,
    pub context: Vec<ContextItem>,
    pub model: Option<String>,
    pub role: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

/// What the session tells the app, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatEvent {
    /// The answer so far (replaces the earlier partial).
    Partial(String),
    Note(String),
    ToolRequest { call: String, tool: String, args: String },
    /// A card: the person decides before the call runs.
    Approval { call: String, tool: String, summary: String, detail: String },
    Decision { call: String, tool: String, approved: bool, by: String },
    /// Run this app tool now and answer it.
    ToolCall { call: String, tool: String, args: String },
    ToolResult { call: String, tool: String, ok: bool, text: String },
    Final(String),
    Failed(String),
    Stopped(String),
}

impl ChatEvent {
    pub fn ends_turn(&self) -> bool {
        matches!(self, ChatEvent::Final(_) | ChatEvent::Failed(_) | ChatEvent::Stopped(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speaker {
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryLine {
    pub who: Speaker,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssistError {
    /// The core refused, in a sentence for the person.
    Refused(String),
    /// This host has no assistant (a preview, a core without one).
    Unavailable,
}

impl AssistError {
    pub fn sentence(&self) -> String {
        match self {
            AssistError::Refused(s) => s.clone(),
            AssistError::Unavailable => "The assistant is not available here.".into(),
        }
    }
}

pub type AssistResult<T> = Result<T, AssistError>;

/// The assistant of this host, for one person and one app. A refusal is a sentence the app shows.
pub trait AssistantPort {
    fn models(&mut self) -> Vec<Choice> {
        Vec::new()
    }
    fn roles(&mut self) -> Vec<Choice> {
        Vec::new()
    }
    /// Open (or reopen) the conversation. `true`: it existed and its history is there.
    fn open(&mut self, _cfg: &SessionConfig) -> AssistResult<bool> {
        Err(AssistError::Unavailable)
    }
    fn set_context(&mut self, _ctx: &[ContextItem]) -> AssistResult<()> {
        Err(AssistError::Unavailable)
    }
    fn choose(&mut self, _model: Option<&str>, _role: Option<&str>) -> AssistResult<()> {
        Err(AssistError::Unavailable)
    }
    fn send(&mut self, _text: &str) -> AssistResult<()> {
        Err(AssistError::Unavailable)
    }
    fn poll(&mut self) -> Vec<ChatEvent> {
        Vec::new()
    }
    fn decide(&mut self, _call: &str, _approve: bool) -> AssistResult<()> {
        Err(AssistError::Unavailable)
    }
    fn answer(&mut self, _call: &str, _ok: bool, _text: &str) -> AssistResult<()> {
        Err(AssistError::Unavailable)
    }
    fn cancel(&mut self) -> AssistResult<()> {
        Err(AssistError::Unavailable)
    }
    fn history(&mut self) -> Vec<HistoryLine> {
        Vec::new()
    }
    fn close(&mut self) {}
}

/// A host with no assistant: the chat view says so instead of failing.
pub struct NoAssistant;

impl AssistantPort for NoAssistant {}

/// One turn of the scripted model: text, and optionally a tool it asks for (the app's plain tool name and JSON arguments).
#[derive(Clone, Debug)]
pub struct Turn {
    pub text: String,
    pub call: Option<(String, String)>,
}

impl Turn {
    pub fn say(text: &str) -> Turn {
        Turn { text: text.into(), call: None }
    }
    pub fn call(text: &str, tool: &str, args: &str) -> Turn {
        Turn { text: text.into(), call: Some((tool.into(), args.into())) }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Idle,
    Approval(String),
    Answer(String),
}

/// The in-memory port: a scripted model, the same approval rules as the core, every decision recorded in `audit`. For tests, the browser
/// preview and the sample's first run; share it with the test through [`ScriptedAssistant::handle`].
#[derive(Default)]
pub struct ScriptedAssistant {
    cfg: Option<SessionConfig>,
    script: VecDeque<Turn>,
    out: VecDeque<ChatEvent>,
    phase: Option<Phase>,
    history: Vec<HistoryLine>,
    pending: BTreeMap<String, (String, String)>,
    n_calls: usize,
    /// What the model was shown at each message: model, role and the context items.
    pub seen: Vec<String>,
    pub audit: Vec<String>,
    pub refuse: Option<String>,
}

impl ScriptedAssistant {
    pub fn new(script: Vec<Turn>) -> ScriptedAssistant {
        ScriptedAssistant { script: script.into(), ..Default::default() }
    }

    pub fn push_script(&mut self, turns: Vec<Turn>) {
        self.script.extend(turns);
    }

    fn advance(&mut self) {
        loop {
            let Some(turn) = self.script.pop_front() else {
                self.out.push_back(ChatEvent::Final("(the scripted model has nothing more to say)".into()));
                self.phase = Some(Phase::Idle);
                return;
            };
            if !turn.text.is_empty() {
                self.out.push_back(ChatEvent::Partial(turn.text.clone()));
            }
            let Some((tool, args)) = turn.call else {
                self.out.push_back(ChatEvent::Final(turn.text.clone()));
                self.history.push(HistoryLine { who: Speaker::Assistant, text: turn.text });
                self.phase = Some(Phase::Idle);
                return;
            };
            self.n_calls += 1;
            let call = format!("call-{}", self.n_calls);
            let spec = self.cfg.as_ref().and_then(|c| c.tools.iter().find(|t| t.name == tool)).cloned();
            let Some(spec) = spec else {
                self.out.push_back(ChatEvent::ToolResult { call, tool: tool.clone(), ok: false, text: format!("unknown tool `{tool}`") });
                continue;
            };
            self.pending.insert(call.clone(), (tool.clone(), args.clone()));
            self.out.push_back(ChatEvent::ToolRequest { call: call.clone(), tool: tool.clone(), args: args.clone() });
            if spec.ask {
                self.out.push_back(ChatEvent::Approval { call: call.clone(), tool, summary: spec.description, detail: args });
                self.phase = Some(Phase::Approval(call));
            } else {
                self.out.push_back(ChatEvent::Decision { call: call.clone(), tool: tool.clone(), approved: true, by: "auto".into() });
                self.out.push_back(ChatEvent::ToolCall { call: call.clone(), tool, args });
                self.phase = Some(Phase::Answer(call));
            }
            return;
        }
    }
}

impl AssistantPort for ScriptedAssistant {
    fn models(&mut self) -> Vec<Choice> {
        vec![Choice { id: "scripted".into(), label: "Scripted model".into() }, Choice { id: "second".into(), label: "Second model".into() }]
    }
    fn roles(&mut self) -> Vec<Choice> {
        vec![Choice { id: "assistant".into(), label: "Assistant".into() }, Choice { id: "concise".into(), label: "Concise".into() }]
    }
    fn open(&mut self, cfg: &SessionConfig) -> AssistResult<bool> {
        if let Some(why) = &self.refuse {
            return Err(AssistError::Refused(why.clone()));
        }
        let resumed = self.cfg.as_ref().is_some_and(|c| c.conversation == cfg.conversation);
        self.audit.push(format!("assistant.open {}", cfg.conversation));
        self.cfg = Some(cfg.clone());
        self.phase.get_or_insert(Phase::Idle);
        Ok(resumed)
    }
    fn set_context(&mut self, ctx: &[ContextItem]) -> AssistResult<()> {
        self.cfg.as_mut().ok_or(AssistError::Unavailable)?.context = ctx.to_vec();
        Ok(())
    }
    fn choose(&mut self, model: Option<&str>, role: Option<&str>) -> AssistResult<()> {
        let c = self.cfg.as_mut().ok_or(AssistError::Unavailable)?;
        if let Some(m) = model {
            c.model = Some(m.into());
        }
        if let Some(r) = role {
            c.role = Some(r.into());
        }
        Ok(())
    }
    fn send(&mut self, text: &str) -> AssistResult<()> {
        if text.trim().is_empty() || text.len() > MAX_TEXT {
            return Err(AssistError::Refused("the message is empty or too long".into()));
        }
        let c = self.cfg.clone().ok_or(AssistError::Unavailable)?;
        if self.phase != Some(Phase::Idle) {
            return Err(AssistError::Refused("the assistant is still answering the last message".into()));
        }
        self.history.push(HistoryLine { who: Speaker::User, text: text.into() });
        self.seen.push(format!("model={} role={} context=[{}]", c.model.as_deref().unwrap_or("default"), c.role.as_deref().unwrap_or("default"), c.context.iter().map(|i| format!("{:?} {}: {}", i.kind, i.name, i.text)).collect::<Vec<_>>().join(" | ")));
        self.advance();
        Ok(())
    }
    fn poll(&mut self) -> Vec<ChatEvent> {
        self.out.drain(..).collect()
    }
    fn decide(&mut self, call: &str, approve: bool) -> AssistResult<()> {
        if self.phase != Some(Phase::Approval(call.to_string())) {
            return Err(AssistError::Refused("no approval is waiting for that call".into()));
        }
        let (tool, args) = self.pending.get(call).cloned().unwrap_or_default();
        self.out.push_back(ChatEvent::Decision { call: call.into(), tool: tool.clone(), approved: approve, by: "user".into() });
        self.audit.push(format!("assistant.tool {tool} {}", if approve { "approved" } else { "denied" }));
        if approve {
            self.out.push_back(ChatEvent::ToolCall { call: call.into(), tool, args });
            self.phase = Some(Phase::Answer(call.into()));
        } else {
            self.out.push_back(ChatEvent::ToolResult { call: call.into(), tool, ok: false, text: "the person did not allow it".into() });
            self.advance();
        }
        Ok(())
    }
    fn answer(&mut self, call: &str, ok: bool, text: &str) -> AssistResult<()> {
        if self.phase != Some(Phase::Answer(call.to_string())) {
            return Err(AssistError::Refused("no tool call is waiting for that answer".into()));
        }
        let (tool, _) = self.pending.get(call).cloned().unwrap_or_default();
        self.audit.push(format!("assistant.tool {tool} ran {}", if ok { "ok" } else { "failed" }));
        self.out.push_back(ChatEvent::ToolResult { call: call.into(), tool, ok, text: text.into() });
        self.history.push(HistoryLine { who: Speaker::Tool, text: text.into() });
        self.advance();
        Ok(())
    }
    fn cancel(&mut self) -> AssistResult<()> {
        if self.phase.as_ref().is_some_and(|p| *p != Phase::Idle) {
            self.out.push_back(ChatEvent::Stopped("stopped by the person".into()));
            self.phase = Some(Phase::Idle);
        }
        Ok(())
    }
    fn history(&mut self) -> Vec<HistoryLine> {
        self.history.clone()
    }
}

/// Lets a test (or the preview) keep a handle on the scripted port the app owns.
pub struct SharedAssistant(pub std::rc::Rc<std::cell::RefCell<ScriptedAssistant>>);

impl SharedAssistant {
    pub fn new(script: Vec<Turn>) -> (SharedAssistant, std::rc::Rc<std::cell::RefCell<ScriptedAssistant>>) {
        let h = std::rc::Rc::new(std::cell::RefCell::new(ScriptedAssistant::new(script)));
        (SharedAssistant(h.clone()), h)
    }
}

macro_rules! forward {
    ($($name:ident($($arg:ident: $t:ty),*) -> $ret:ty;)*) => {
        $(fn $name(&mut self, $($arg: $t),*) -> $ret { self.0.borrow_mut().$name($($arg),*) })*
    };
}

impl AssistantPort for SharedAssistant {
    forward! {
        models() -> Vec<Choice>;
        roles() -> Vec<Choice>;
        open(cfg: &SessionConfig) -> AssistResult<bool>;
        set_context(ctx: &[ContextItem]) -> AssistResult<()>;
        choose(model: Option<&str>, role: Option<&str>) -> AssistResult<()>;
        send(text: &str) -> AssistResult<()>;
        poll() -> Vec<ChatEvent>;
        decide(call: &str, approve: bool) -> AssistResult<()>;
        answer(call: &str, ok: bool, text: &str) -> AssistResult<()>;
        cancel() -> AssistResult<()>;
        history() -> Vec<HistoryLine>;
    }
    fn close(&mut self) {
        self.0.borrow_mut().close()
    }
}
