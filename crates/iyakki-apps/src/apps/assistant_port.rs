//! The SDK's `AssistantPort` over the core's assistant facet (`cx.core.assistant()`), for one person and one app: what an SDK app gets so
//! that it can embed the assistant (`iyakki_ui_sdk::chatview::ChatView`). The port only maps types; the core decides everything (the
//! account's agent switch, the model, the approvals, the audit lines), so a refusal arrives here as a sentence and is handed to the app.
//! `AuditLogSink` lets the core write its lines into the machine's audit log.

use std::sync::Arc;

use iyakki_core_api::assistant as api;
use iyakki_core_api::{AuditSink, Core};
use iyakki_ui_sdk::assistant::{AssistError, AssistResult, AssistantPort, ChatEvent, Choice, ContextItem, ContextKind, HistoryLine, SessionConfig, Speaker};


/// The machine's audit log as the core's audit sink.
pub struct AuditLogSink(pub Arc<dyn iyakki_agent_api::AuditApi>);

impl AuditSink for AuditLogSink {
    fn record(&self, user: &str, event: &str, detail: &str) {
        self.0.record(user, event, detail);
    }
}

pub struct CoreAssistantPort {
    core: Arc<dyn Core>,
    user: String,
    app: String,
    session: Option<String>,
}

impl CoreAssistantPort {
    pub fn new(core: Arc<dyn Core>, user: &str, app: &str) -> CoreAssistantPort {
        CoreAssistantPort { core, user: user.into(), app: app.into(), session: None }
    }

    fn id(&self) -> AssistResult<&str> {
        self.session.as_deref().ok_or(AssistError::Unavailable)
    }
}

fn refused(e: String) -> AssistError {
    if e.contains("no embedded assistant") { AssistError::Unavailable } else { AssistError::Refused(e) }
}

fn kind(k: ContextKind) -> api::ContextKind {
    match k {
        ContextKind::Document => api::ContextKind::Document,
        ContextKind::Form => api::ContextKind::Form,
        ContextKind::Selection => api::ContextKind::Selection,
        ContextKind::Other => api::ContextKind::Other,
    }
}

fn context(items: &[ContextItem]) -> Vec<api::ContextItem> {
    items.iter().map(|c| api::ContextItem { kind: kind(c.kind), name: c.name.clone(), text: c.text.clone() }).collect()
}

fn choices(v: Vec<api::Choice>) -> Vec<Choice> {
    v.into_iter().map(|c| Choice { id: c.id, label: c.label }).collect()
}

fn event(e: api::AssistEvent) -> ChatEvent {
    use api::AssistEvent as E;
    match e {
        E::Partial { text } => ChatEvent::Partial(text),
        E::Note { text } => ChatEvent::Note(text),
        E::ToolRequest { call, tool, args } => ChatEvent::ToolRequest { call, tool, args },
        E::Approval { call, tool, summary, detail } => ChatEvent::Approval { call, tool, summary, detail },
        E::Decision { call, tool, approved, by } => ChatEvent::Decision { call, tool, approved, by },
        E::ToolCall { call, tool, args } => ChatEvent::ToolCall { call, tool, args },
        E::ToolResult { call, tool, ok, text } => ChatEvent::ToolResult { call, tool, ok, text },
        E::Final { text } => ChatEvent::Final(text),
        E::Failed { error } => ChatEvent::Failed(error),
        E::Stopped { reason } => ChatEvent::Stopped(reason),
    }
}

impl AssistantPort for CoreAssistantPort {
    fn models(&mut self) -> Vec<Choice> {
        choices(self.core.assistant().models(&self.user))
    }

    fn roles(&mut self) -> Vec<Choice> {
        choices(self.core.assistant().roles(&self.user))
    }

    fn open(&mut self, cfg: &SessionConfig) -> AssistResult<bool> {
        let spec = api::SessionSpec {
            conversation: cfg.conversation.clone(),
            title: cfg.title.clone(),
            tools: cfg
                .tools
                .iter()
                .map(|t| api::AppToolSpec { name: t.name.clone(), description: t.description.clone(), schema: t.schema.clone(), approval: if t.ask { api::ToolApproval::Ask } else { api::ToolApproval::Safe } })
                .collect(),
            context: context(&cfg.context),
            model: cfg.model.clone(),
            role: cfg.role.clone(),
        };
        let info = self.core.assistant().open(&self.user, &self.app, &spec).map_err(refused)?;
        self.session = Some(info.id);
        Ok(info.resumed)
    }

    fn set_context(&mut self, ctx: &[ContextItem]) -> AssistResult<()> {
        self.core.assistant().set_context(&self.user, self.id()?, &context(ctx)).map_err(refused)
    }

    fn choose(&mut self, model: Option<&str>, role: Option<&str>) -> AssistResult<()> {
        self.core.assistant().choose(&self.user, self.id()?, model, role).map_err(refused)
    }

    fn send(&mut self, text: &str) -> AssistResult<()> {
        self.core.assistant().send(&self.user, self.id()?, text).map_err(refused)
    }

    fn poll(&mut self) -> Vec<ChatEvent> {
        match self.id() {
            Ok(id) => self.core.assistant().poll(&self.user, id).into_iter().map(event).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn decide(&mut self, call: &str, approve: bool) -> AssistResult<()> {
        self.core.assistant().decide(&self.user, self.id()?, call, approve).map_err(refused)
    }

    fn answer(&mut self, call: &str, ok: bool, text: &str) -> AssistResult<()> {
        self.core.assistant().answer(&self.user, self.id()?, call, ok, text).map_err(refused)
    }

    fn cancel(&mut self) -> AssistResult<()> {
        self.core.assistant().cancel(&self.user, self.id()?).map_err(refused)
    }

    fn history(&mut self) -> Vec<HistoryLine> {
        let Ok(id) = self.id() else { return Vec::new() };
        self.core
            .assistant()
            .history(&self.user, id)
            .into_iter()
            .map(|h| HistoryLine { who: match h.role { api::HistoryRole::User => Speaker::User, api::HistoryRole::Assistant => Speaker::Assistant, api::HistoryRole::Tool => Speaker::Tool }, text: h.text })
            .collect()
    }

    fn close(&mut self) {
        if let Some(id) = self.session.take() {
            self.core.assistant().close(&self.user, &id);
        }
    }
}

impl Drop for CoreAssistantPort {
    fn drop(&mut self) {
        self.close();
    }
}
