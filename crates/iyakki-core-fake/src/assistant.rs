//! The fake embedded assistant: a scripted model, no network, every decision recorded. It follows the same rules as the real one (limits
//! from `iyakki_core_api::assistant`, a session belongs to one person, an app tool that asks waits for the person, a tool the app did not
//! declare is a failure the model is told) so a test of an SDK app against it means the same thing as against the device.

use iyakki_core_api::assistant::{check_context, check_message, check_spec, AppToolSpec, AssistEvent, Assistant, Choice, ContextItem, HistoryEntry, HistoryRole, SessionInfo, SessionSpec, ToolApproval};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

/// One turn of the scripted model: some text, and optionally a tool it asks for (the app's plain tool name and the arguments as JSON text).
#[derive(Clone, Debug)]
pub struct FakeTurn {
    pub text: String,
    pub call: Option<(String, String)>,
}

impl FakeTurn {
    pub fn say(text: &str) -> FakeTurn {
        FakeTurn { text: text.into(), call: None }
    }
    pub fn call(text: &str, tool: &str, args: &str) -> FakeTurn {
        FakeTurn { text: text.into(), call: Some((tool.into(), args.into())) }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Idle,
    Running,
    Approval(String),
    Answer(String),
}

struct Session {
    user: String,
    app: String,
    conversation: String,
    tools: Vec<AppToolSpec>,
    context: Vec<ContextItem>,
    model: Option<String>,
    role: Option<String>,
    out: VecDeque<AssistEvent>,
    phase: Phase,
    history: Vec<HistoryEntry>,
    n_calls: usize,
    open: bool,
    /// The call that waits (for the person or the app): its tool name and arguments.
    pending: BTreeMap<String, (String, String)>,
}

impl Session {
    fn pending_of(&self, call: &str) -> (String, String) {
        self.pending.get(call).cloned().unwrap_or_default()
    }
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<String, Session>,
    script: VecDeque<FakeTurn>,
    audit: Vec<String>,
    /// What the model was shown at each `send`: the context items as `kind name: text`, the model and the role.
    seen: Vec<String>,
    next: u32,
    refuse_open: Option<String>,
}

#[derive(Default)]
pub struct FakeAssistant {
    st: Mutex<State>,
}

fn lock(m: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl FakeAssistant {
    pub fn new() -> FakeAssistant {
        FakeAssistant::default()
    }

    /// The turns the model will take, in order, over all the sessions (what a real model would decide).
    pub fn script(&self, turns: Vec<FakeTurn>) {
        lock(&self.st).script = turns.into();
    }

    /// The audit lines the core wrote (`assistant.open sample main`, `assistant.tool sample add_note approved by user`...).
    pub fn audit(&self) -> Vec<String> {
        lock(&self.st).audit.clone()
    }

    /// What the model was shown at each message.
    pub fn seen(&self) -> Vec<String> {
        lock(&self.st).seen.clone()
    }

    pub fn refuse_open(&self, why: Option<&str>) {
        lock(&self.st).refuse_open = why.map(str::to_string);
    }
}

impl State {
    fn advance(&mut self, id: &str) {
        loop {
            let Some(turn) = self.script.pop_front() else {
                let s = self.sessions.get_mut(id).expect("session");
                s.out.push_back(AssistEvent::Final { text: "(the scripted model has nothing more to say)".into() });
                s.phase = Phase::Idle;
                return;
            };
            let s = self.sessions.get_mut(id).expect("session");
            if !turn.text.is_empty() {
                s.out.push_back(AssistEvent::Partial { text: turn.text.clone() });
            }
            let Some((tool, args)) = turn.call else {
                s.out.push_back(AssistEvent::Final { text: turn.text.clone() });
                s.history.push(HistoryEntry { role: HistoryRole::Assistant, text: turn.text });
                s.phase = Phase::Idle;
                return;
            };
            s.n_calls += 1;
            let call = format!("call-{}", s.n_calls);
            let Some(spec) = s.tools.iter().find(|t| t.name == tool).cloned() else {
                s.out.push_back(AssistEvent::ToolResult { call, tool: tool.clone(), ok: false, text: format!("unknown tool `{tool}`") });
                continue;
            };
            s.pending.insert(call.clone(), (tool.clone(), args.clone()));
            s.out.push_back(AssistEvent::ToolRequest { call: call.clone(), tool: tool.clone(), args: args.clone() });
            match spec.approval {
                ToolApproval::Ask => {
                    s.out.push_back(AssistEvent::Approval { call: call.clone(), tool: tool.clone(), summary: format!("{}: {}", s.app, spec.description), detail: args });
                    s.phase = Phase::Approval(call);
                }
                ToolApproval::Safe => {
                    s.out.push_back(AssistEvent::Decision { call: call.clone(), tool: tool.clone(), approved: true, by: "auto".into() });
                    s.out.push_back(AssistEvent::ToolCall { call: call.clone(), tool, args });
                    s.phase = Phase::Answer(call);
                }
            }
            return;
        }
    }
}

impl Assistant for FakeAssistant {
    fn models(&self, _: &str) -> Vec<Choice> {
        vec![Choice { id: "scripted".into(), label: "Scripted test model".into() }, Choice { id: "second".into(), label: "Second test model".into() }]
    }

    fn roles(&self, _: &str) -> Vec<Choice> {
        vec![Choice { id: "assistant".into(), label: "Assistant".into() }, Choice { id: "concise".into(), label: "Concise".into() }]
    }

    fn open(&self, user: &str, app: &str, spec: &SessionSpec) -> Result<SessionInfo, String> {
        check_spec(app, spec)?;
        let mut st = lock(&self.st);
        if let Some(why) = st.refuse_open.clone() {
            return Err(why);
        }
        let existing = st.sessions.iter().find(|(_, s)| s.user == user && s.app == app && s.conversation == spec.conversation).map(|(k, _)| k.clone());
        st.audit.push(format!("assistant.open {app} {}", spec.conversation));
        if let Some(id) = existing {
            let s = st.sessions.get_mut(&id).unwrap();
            s.tools = spec.tools.clone();
            s.context = spec.context.clone();
            s.open = true;
            return Ok(SessionInfo { id, resumed: true });
        }
        st.next += 1;
        let id = format!("fake-session-{}", st.next);
        st.sessions.insert(id.clone(), Session { user: user.into(), app: app.into(), conversation: spec.conversation.clone(), tools: spec.tools.clone(), context: spec.context.clone(), model: spec.model.clone(), role: spec.role.clone(), out: VecDeque::new(), phase: Phase::Idle, history: Vec::new(), n_calls: 0, open: true, pending: BTreeMap::new() });
        Ok(SessionInfo { id, resumed: false })
    }

    fn set_context(&self, user: &str, session: &str, context: &[ContextItem]) -> Result<(), String> {
        check_context(context)?;
        let mut st = lock(&self.st);
        let s = st.sessions.get_mut(session).filter(|s| s.user == user).ok_or("no such session")?;
        s.context = context.to_vec();
        Ok(())
    }

    fn choose(&self, user: &str, session: &str, model: Option<&str>, role: Option<&str>) -> Result<(), String> {
        let models = self.models(user);
        let roles = self.roles(user);
        if model.is_some_and(|m| !models.iter().any(|c| c.id == m)) {
            return Err("that model is not one of the choices".into());
        }
        if role.is_some_and(|r| !roles.iter().any(|c| c.id == r)) {
            return Err("that role is not one of the choices".into());
        }
        let mut st = lock(&self.st);
        let s = st.sessions.get_mut(session).filter(|s| s.user == user).ok_or("no such session")?;
        if let Some(m) = model {
            s.model = Some(m.into());
        }
        if let Some(r) = role {
            s.role = Some(r.into());
        }
        Ok(())
    }

    fn send(&self, user: &str, session: &str, text: &str) -> Result<(), String> {
        check_message(text)?;
        let mut st = lock(&self.st);
        let s = st.sessions.get_mut(session).filter(|s| s.user == user && s.open).ok_or("no such session")?;
        if s.phase != Phase::Idle {
            return Err("the assistant is still answering the last message".into());
        }
        s.history.push(HistoryEntry { role: HistoryRole::User, text: text.into() });
        s.phase = Phase::Running;
        let shown = format!("model={} role={} context=[{}]", s.model.as_deref().unwrap_or("default"), s.role.as_deref().unwrap_or("default"), s.context.iter().map(|c| format!("{} {}: {}", c.kind.word(), c.name, c.text)).collect::<Vec<_>>().join(" | "));
        st.seen.push(shown);
        st.advance(session);
        Ok(())
    }

    fn poll(&self, user: &str, session: &str) -> Vec<AssistEvent> {
        let mut st = lock(&self.st);
        match st.sessions.get_mut(session).filter(|s| s.user == user) {
            Some(s) => s.out.drain(..).collect(),
            None => Vec::new(),
        }
    }

    fn decide(&self, user: &str, session: &str, call: &str, approve: bool) -> Result<(), String> {
        let mut st = lock(&self.st);
        let s = st.sessions.get_mut(session).filter(|s| s.user == user).ok_or("no such session")?;
        if s.phase != Phase::Approval(call.to_string()) {
            return Err("no approval is waiting for that call".into());
        }
        let app = s.app.clone();
        let (tool, args) = s.pending_of(call);
        s.out.push_back(AssistEvent::Decision { call: call.into(), tool: tool.clone(), approved: approve, by: "user".into() });
        if approve {
            s.out.push_back(AssistEvent::ToolCall { call: call.into(), tool: tool.clone(), args });
            s.phase = Phase::Answer(call.into());
        } else {
            s.out.push_back(AssistEvent::ToolResult { call: call.into(), tool: tool.clone(), ok: false, text: "the person did not allow it".into() });
            s.phase = Phase::Running;
        }
        st.audit.push(format!("assistant.tool {app} {tool} {} by user", if approve { "approved" } else { "denied" }));
        if !approve {
            st.advance(session);
        }
        Ok(())
    }

    fn answer(&self, user: &str, session: &str, call: &str, ok: bool, text: &str) -> Result<(), String> {
        let mut st = lock(&self.st);
        let s = st.sessions.get_mut(session).filter(|s| s.user == user).ok_or("no such session")?;
        if s.phase != Phase::Answer(call.to_string()) {
            return Err("no tool call is waiting for that answer".into());
        }
        let (tool, _) = s.pending_of(call);
        s.out.push_back(AssistEvent::ToolResult { call: call.into(), tool: tool.clone(), ok, text: text.into() });
        s.history.push(HistoryEntry { role: HistoryRole::Tool, text: text.into() });
        s.phase = Phase::Running;
        let app = s.app.clone();
        st.audit.push(format!("assistant.tool {app} {tool} ran {}", if ok { "ok" } else { "failed" }));
        st.advance(session);
        Ok(())
    }

    fn cancel(&self, user: &str, session: &str) -> Result<(), String> {
        let mut st = lock(&self.st);
        let s = st.sessions.get_mut(session).filter(|s| s.user == user).ok_or("no such session")?;
        if s.phase != Phase::Idle {
            s.out.push_back(AssistEvent::Stopped { reason: "stopped by the person".into() });
            s.phase = Phase::Idle;
        }
        Ok(())
    }

    fn history(&self, user: &str, session: &str) -> Vec<HistoryEntry> {
        lock(&self.st).sessions.get(session).filter(|s| s.user == user).map(|s| s.history.clone()).unwrap_or_default()
    }

    fn close(&self, user: &str, session: &str) {
        if let Some(s) = lock(&self.st).sessions.get_mut(session).filter(|s| s.user == user) {
            s.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::assistant::ToolApproval;

    fn spec() -> SessionSpec {
        SessionSpec {
            conversation: "c".into(),
            tools: vec![AppToolSpec { name: "add".into(), description: "Add".into(), schema: "{\"type\":\"object\"}".into(), approval: ToolApproval::Ask }],
            ..Default::default()
        }
    }

    #[test]
    fn the_fake_follows_the_core_rules_for_approval_answers_owners_and_cancel() {
        let a = FakeAssistant::new();
        a.script(vec![FakeTurn::call("", "add", "{}"), FakeTurn::say("done"), FakeTurn::call("", "add", "{}")]);
        let s = a.open("u", "app", &spec()).unwrap();
        assert!(a.open("v", "app", &spec()).unwrap().id != s.id, "another person gets a session of their own");
        a.send("u", &s.id, "go").unwrap();
        assert!(a.send("u", &s.id, "again").is_err(), "busy");
        assert!(a.answer("u", &s.id, "call-1", true, "x").is_err(), "no answer before the person decided");
        assert!(a.decide("v", &s.id, "call-1", true).is_err(), "not the owner");
        a.decide("u", &s.id, "call-1", true).unwrap();
        a.answer("u", &s.id, "call-1", true, "ok").unwrap();
        let ev = a.poll("u", &s.id);
        assert!(matches!(ev.last(), Some(AssistEvent::Final { text }) if text == "done"), "{ev:?}");
        a.send("u", &s.id, "more").unwrap();
        a.cancel("u", &s.id).unwrap();
        assert!(matches!(a.poll("u", &s.id).last(), Some(AssistEvent::Stopped { .. })));
        assert_eq!(a.audit()[0], "assistant.open app c");
        assert!(a.audit().iter().any(|l| l == "assistant.tool app add approved by user"));
        assert_eq!(a.history("u", &s.id).first().map(|h| h.text.as_str()), Some("go"));
    }
}
