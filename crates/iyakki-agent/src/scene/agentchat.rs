//! An external agent (Claude Code and the like) used as a plain text model for a picture: one request in, one text out,
//! no tools, an empty working folder. TALES uses it so the pictures are drawn by the same writer the person chose for the
//! story, not by a smaller model somewhere else.

use std::path::PathBuf;
use std::sync::Arc;

use zeroize::Zeroizing;

use crate::chat::{ChatBackend, Message, Role};
use crate::extagent::bridge::Audit;
use crate::extagent::run::{start, RunRequest};
use crate::extagent::{ExternalAgent, NoTools};

pub struct AgentChat {
    pub agent: Arc<dyn ExternalAgent>,
    pub workspace: PathBuf,
    pub model: String,
    pub key: Option<Zeroizing<String>>,
    pub audit: Audit,
    pub on_usage: Arc<dyn Fn(&str, &str, u64, u64) + Send + Sync>,
    pub exe: PathBuf,
}

impl ChatBackend for AgentChat {
    fn complete(&self, history: &[Message]) -> Result<String, String> {
        let system = history.iter().filter(|m| m.role == Role::System).map(|m| m.content.clone()).collect::<Vec<_>>().join("\n\n");
        let prompt = history.iter().filter(|m| m.role != Role::System).map(|m| m.content.clone()).collect::<Vec<_>>().join("\n\n");
        let req = RunRequest {
            agent: self.agent.clone(),
            workspace: self.workspace.clone(),
            prompt,
            system: Arc::new(move || system.clone()),
            model: self.model.clone(),
            resume: None,
            tools: Arc::new(NoTools),
            limits: crate::agent::RunLimits { max_seconds: 600, max_steps: 1, ..Default::default() },
            allow_auto: false,
            key: self.key.clone(),
            audit: self.audit.clone(),
            exe: self.exe.clone(),
            on_session: Arc::new(|_| {}),
            on_usage: self.on_usage.clone(),
            no_tools: true,
        };
        let h = start(req);
        let text = crate::story::follow_agent_run(&h, &|| false, &mut |_| {}, &mut |_| {})?;
        if text.trim().is_empty() {
            Err("the agent sent no answer".into())
        } else {
            Ok(text)
        }
    }
}
