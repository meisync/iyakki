//! What the RUNS tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`runs.rs`) names no `Services`, no session and no audit log: it asks a [`RunsPort`] for the signed-in person's runs. The real port
//! ([`CxPort`]) is the only place that reads the person's name from the session, calls the core's run store for that person (the store never
//! shows another person's runs) and writes the audit lines.

use iyakki_shared::runs::{Integrity, RunLog, RunMeta};

use super::AppCx;

pub trait RunsPort {
    /// The recorded runs of the signed-in person, newest first.
    fn list(&self) -> Vec<RunMeta>;
    /// One run's record and whether its hash chain is intact.
    fn load(&self, id: &str) -> Option<(RunLog, Integrity)>;
    /// Delete one of the person's runs.
    fn delete(&self, id: &str) -> Result<(), String>;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl RunsPort for CxPort<'_> {
    fn list(&self) -> Vec<RunMeta> {
        self.0.runs().list(&self.0.session.user)
    }
    fn load(&self, id: &str) -> Option<(RunLog, Integrity)> {
        self.0.runs().load(&self.0.session.user, id).ok()
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.0.runs().delete(&self.0.session.user, id)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
}

// ---- the tile crate's ports (D-S1-E4-01, option (b), D-S1-E5-01): plain mirrors in iyakki-tile-api, the records stay on this side ----------

use iyakki_shared::agent::RunEvent;
use iyakki_tile_api::runs as api;

use super::jobs_ui::JobsCx;

pub fn to_meta(m: &RunMeta) -> api::RunMeta {
    api::RunMeta { id: m.id.clone(), started: m.started, prompt: m.prompt.clone(), status: m.status.clone(), tool_calls: m.tool_calls, task_id: m.task_id.clone(), row: m.row.clone(), agent: m.agent.clone() }
}

pub fn to_event(e: &RunEvent) -> api::RunEvent {
    match e {
        RunEvent::Model { text, calls } => api::RunEvent::Model { text: text.clone(), calls: calls.iter().map(|c| api::Call { name: c.name.clone(), args_text: c.args.to_string() }).collect() },
        RunEvent::Approval { summary, detail, .. } => api::RunEvent::Approval { summary: summary.clone(), detail: detail.clone() },
        RunEvent::Decision { name, approved, by, .. } => api::RunEvent::Decision { name: name.clone(), approved: *approved, by: by.clone() },
        RunEvent::ToolResult { name, ok, text, .. } => api::RunEvent::ToolResult { name: name.clone(), ok: *ok, text: text.clone() },
        RunEvent::Usage { tokens, cost_cents, files, seconds } => api::RunEvent::Usage { tokens: *tokens, cost_cents: *cost_cents, files: *files, seconds: *seconds },
        RunEvent::Note { text } => api::RunEvent::Note { text: text.clone() },
        RunEvent::Timing { first_token_ms, first_text_ms, total_ms, model_ms, tools_ms, overhead_ms, thinking_ms, .. } => {
            api::RunEvent::Timing { first_token_ms: *first_token_ms, first_text_ms: *first_text_ms, total_ms: *total_ms, model_ms: *model_ms, tools_ms: *tools_ms, overhead_ms: *overhead_ms, thinking_ms: *thinking_ms }
        }
        RunEvent::Final { text } => api::RunEvent::Final { text: text.clone() },
        RunEvent::Failed { error } => api::RunEvent::Failed { error: error.clone() },
        RunEvent::Stopped { reason } => api::RunEvent::Stopped { reason: reason.clone() },
        RunEvent::Delta { .. } | RunEvent::Thinking { .. } | RunEvent::Budget { .. } => api::RunEvent::Hidden,
    }
}

pub fn to_log(l: &RunLog) -> api::RunLog {
    api::RunLog { id: l.id.clone(), prompt: l.prompt.clone(), events: l.events.iter().map(to_event).collect() }
}

pub fn to_integrity(i: &Integrity) -> api::Integrity {
    match i {
        Integrity::Verified => api::Integrity::Verified,
        Integrity::Tampered(w) => api::Integrity::Tampered(w.clone()),
    }
}

enum Src<'a> {
    Cx(CxPort<'a>),
    Dyn(&'a dyn RunsPort),
}

/// The tile's ports over the shell: the run store through the shell's [`RunsPort`] and, when the shell's context is there, the jobs.
pub struct Bridge<'a> {
    runs: Src<'a>,
    cx: Option<&'a AppCx>,
}

impl<'a> Bridge<'a> {
    pub fn new(cx: &'a AppCx) -> Bridge<'a> {
        Bridge { runs: Src::Cx(CxPort(cx)), cx: Some(cx) }
    }
    /// Over a run port alone (a fake in tests, the first read when the tile opens): there are no jobs.
    pub fn runs_only(port: &'a dyn RunsPort) -> Bridge<'a> {
        Bridge { runs: Src::Dyn(port), cx: None }
    }
    fn p(&self) -> &dyn RunsPort {
        match &self.runs {
            Src::Cx(c) => c,
            Src::Dyn(d) => *d,
        }
    }
}

impl api::RunsPort for Bridge<'_> {
    fn list(&self) -> Vec<api::RunMeta> {
        self.p().list().iter().map(to_meta).collect()
    }
    fn load(&self, id: &str) -> Option<(api::RunLog, api::Integrity)> {
        self.p().load(id).map(|(l, i)| (to_log(&l), to_integrity(&i)))
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.p().delete(id)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.p().audit(action, subject)
    }
}

impl api::JobsPort for Bridge<'_> {
    fn visible(&self, ws: Option<&str>) -> Vec<api::Job> {
        self.cx.map(|c| api::JobsPort::visible(&JobsCx(c), ws)).unwrap_or_default()
    }
    fn take_finished(&self, ws: &str) -> Vec<String> {
        self.cx.map(|c| api::JobsPort::take_finished(&JobsCx(c), ws)).unwrap_or_default()
    }
    fn log_len(&self, id: &str) -> u64 {
        self.cx.map_or(0, |c| api::JobsPort::log_len(&JobsCx(c), id))
    }
    fn log_text(&self, id: &str, max: usize) -> Result<String, String> {
        match self.cx {
            Some(c) => api::JobsPort::log_text(&JobsCx(c), id, max),
            None => Err("there are no jobs here".into()),
        }
    }
    fn stop_job(&self, id: &str) -> Box<dyn FnOnce() + Send> {
        match self.cx {
            Some(c) => api::JobsPort::stop_job(&JobsCx(c), id),
            None => Box::new(|| {}),
        }
    }
    fn dismiss(&self, id: &str) -> Result<(), String> {
        match self.cx {
            Some(c) => api::JobsPort::dismiss(&JobsCx(c), id),
            None => Err("there are no jobs here".into()),
        }
    }
    fn audit(&self, action: &str, subject: &str) {
        self.p().audit(action, subject)
    }
}
