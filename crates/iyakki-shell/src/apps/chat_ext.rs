//! External coding agents in the chat: which one answers this chat, how a run with one is started, and the footer line.
//! The agents themselves live in `iyakki_agent::extagent`; Settings → Agents configures them.

use super::*;
use iyakki_agent::extagent::{self, run::RunRequest};

impl ChatApp {
    /// The agent that answers this chat (`None`: the built-in model of Settings → Models). The first time it is asked, the
    /// workspace's remembered choice (`.iyakki/agent`) or the system default applies.
    pub(super) fn ext_id(&mut self, cx: &AppCx) -> Option<String> {
        if self.ext_choice.is_none() {
            let remembered = self.project.as_ref().and_then(|p| std::fs::read_to_string(p.root.join(".iyakki/agent")).ok()).map(|s| s.trim().to_string());
            let cfg = cx.services.agents_cfg();
            // the file holds the agent id and, after a space, the model chosen with it
            let (remembered, model) = match remembered.as_deref().map(|r| r.split_once(' ').map_or((r, None), |(a, m)| (a, Some(m.trim())))) {
                Some((a, m)) => (Some(a.to_string()), m.filter(|m| extagent::valid_model_id(m)).map(str::to_string)),
                None => (None, None),
            };
            let id = match remembered {
                Some(r) if r == "-" => None,
                Some(r) if !r.is_empty() => {
                    self.ext_model = model;
                    Some(r)
                }
                _ => cfg.default.clone(),
            };
            self.ext_choice = Some(id.filter(|id| cfg.agent(id).is_some()));
        }
        self.ext_choice.clone().flatten()
    }

    /// The model used with `agent` in this chat: the one chosen here, else the agent's own setting (empty: the CLI chooses).
    pub(super) fn ext_model_of(&self, cx: &AppCx, agent: &str) -> String {
        match &self.ext_model {
            Some(m) => m.clone(),
            None => cx.services.agents_cfg().agent(agent).map(|a| a.cfg_model()).unwrap_or_default(),
        }
    }

    fn remember_agent(&self, cx: &AppCx, id: Option<&str>) {
        if let Some(p) = &self.project {
            let _ = std::fs::create_dir_all(p.root.join(".iyakki"));
            let line = match (id, &self.ext_model) {
                (Some(i), Some(m)) if !m.is_empty() => format!("{i} {m}"),
                (Some(i), _) => i.to_string(),
                (None, _) => "-".to_string(),
            };
            let _ = std::fs::write(p.root.join(".iyakki/agent"), line);
        } else if cx.session.policy.manage_settings {
            let _ = cx.services.settings.lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.agents.default = id.map(str::to_string));
        }
    }

    /// The person picked a built-in model: this chat (and new chats of the workspace) use it again.
    pub(super) fn choose_builtin(&mut self, cx: &AppCx) {
        self.ext_choice = Some(None);
        self.ext_model = None;
        self.remember_agent(cx, None);
    }

    /// An agent and one of its models were picked (`custom`: the id was typed, so it is remembered for the agent).
    pub(super) fn choose_agent_model(&mut self, cx: &AppCx, agent: &str, model: String, custom: bool) {
        // another agent starts with its own setting unless a model is given here
        self.ext_model = Some(model.clone());
        if custom {
            super::model_picker::remember_custom(cx, agent, &model);
        }
        self.choose_agent(cx, agent.to_string());
    }

    /// The person picked an agent in the model list. An agent that is not ready is still chosen; the exact fix is shown.
    pub(super) fn choose_agent(&mut self, cx: &AppCx, id: String) {
        let p = &cx.session.policy;
        if p.cli != iyakki_sec::policy::CliLevel::Full {
            self.error = Some("Your account may not run commands, so external coding agents are off for it.".into());
            return;
        }
        self.ext_choice = Some(Some(id.clone()));
        self.remember_agent(cx, Some(&id));
        cx.audit.record(&cx.session.user, "chat.agent", &id);
        let cfg = cx.services.agents_cfg();
        match cx.services.ext_status.get(&id) {
            Some(st) if !st.ready() => self.error = Some(st.fix),
            Some(_) => self.error = None,
            None => {
                cx.services.ext_status.refresh(cfg);
                self.error = None;
            }
        }
    }

    /// "agent: Claude Code · sonnet · session 1a2b3c4d" for the footer.
    pub(super) fn ext_footer(&mut self, cx: &AppCx) -> Option<String> {
        let id = self.ext_id(cx)?;
        let cfg = cx.services.agents_cfg();
        let a = cfg.agent(&id)?;
        let chosen = self.ext_model_of(cx, &id);
        let model = if chosen.is_empty() || chosen == "default" { "Default".to_string() } else { cx.services.ext_status.models_for(a.as_ref()).into_iter().find(|m| m.id == chosen).map_or(chosen.clone(), |m| m.label.split(" (").next().unwrap_or(&m.label).to_string()) };
        let sess = self.session.as_ref().and_then(|s| cx.services.ext_sessions.get(&s.id, &id)).map_or("new session".to_string(), |s| format!("session {}", s.chars().take(8).collect::<String>()));
        let state = cx.services.ext_status.get(&id).map_or(String::new(), |s| if s.ready() { String::new() } else { format!(" · {}", s.chip) });
        Some(format!("agent: {} · {model} · {sess}{state} · {}", a.label(), a.mode().label()))
    }

    /// Start a run with the chosen agent: same tools, approvals and policy as the built-in assistant (through the bridge).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_external(&mut self, cx: &AppCx, id: &str, text: &str, tools: Arc<dyn iyakki_agent::agent::Tools>, limits: RunLimits, prepare: Option<iyakki_agent::assistrun::Prepare>, workspace: Option<std::path::PathBuf>) -> Result<RunSession, String> {
        let sid = self.session.as_ref().map(|s| s.id.clone());
        let model = self.ext_model_of(cx, id);
        let req = make_request(cx, id, text, tools, limits, prepare, workspace, sid, model)?;
        let h = extagent::run::start(req);
        Ok(RunSession::from_handle(Some(cx.services.runs.clone()), &cx.session.user, text, h))
    }

    /// Why Autopilot cannot drive this agent (`None`: it can). Autopilot needs an agent that can use IYAKKI's tools.
    pub(super) fn autopilot_refusal(&mut self, cx: &AppCx) -> Option<String> {
        let id = self.ext_id(cx)?;
        let a = cx.services.agents_cfg().agent(&id)?;
        (a.mode() != extagent::Mode::Bridged).then(|| if id == extagent::COPILOT {
            "Autopilot needs an agent that works through IYAKKI's tools and approvals. GitHub Copilot normally does, but it is set to advisor only (read-only) in Settings → Agents: switch Advisor only off there, or pick a built-in model or Claude Code (Ctrl+P), or turn Autopilot off to chat with it step by step.".to_string()
        } else {
            format!("Autopilot needs an agent that can use IYAKKI's tools; Claude Code and GitHub Copilot can, {} runs read-only here (it can only advise). Pick a built-in model, Claude Code or GitHub Copilot (Ctrl+P) for Autopilot, or turn Autopilot off to chat with {} step by step.", a.label(), a.label())
        })
    }

    /// The Autopilot steps carried out by the agent: each step's message is its prompt, resuming its own session, so it keeps
    /// its context; the harness keeps the board, the checks, the caps and the summary.
    pub(super) fn ext_executor(&self, cx: &AppCx, id: &str, tools: Arc<dyn iyakki_agent::agent::Tools>, workspace: Option<std::path::PathBuf>) -> iyakki_agent::autopilot::Executor {
        let (cx, id, sid, model) = (cx.clone(), id.to_string(), self.session.as_ref().map(|s| s.id.clone()), self.ext_model_of(cx, id));
        Arc::new(move |call: iyakki_agent::autopilot::StepCall| {
            match make_request(&cx, &id, &call.prompt, tools.clone(), call.limits, call.prepare, workspace.clone(), sid.clone(), model.clone()) {
                Ok(req) => extagent::run::start(req),
                Err(e) => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let (dtx, _drx) = std::sync::mpsc::channel();
                    let _ = tx.send(RunEvent::Failed { error: e });
                    iyakki_agent::agent::RunHandle::from_parts(rx, dtx)
                }
            }
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn make_request(cx: &AppCx, id: &str, text: &str, tools: Arc<dyn iyakki_agent::agent::Tools>, limits: RunLimits, prepare: Option<iyakki_agent::assistrun::Prepare>, workspace: Option<std::path::PathBuf>, sid: Option<String>, model: String) -> Result<RunRequest, String> {
    let cfg = cx.services.agents_cfg();
    let agent: Arc<dyn extagent::ExternalAgent> = Arc::from(cfg.agent(id).ok_or("That agent is no longer configured. Pick a model again (Ctrl+P).")?);
    let workspace = workspace.ok_or("This chat has no work folder to run an agent in.")?;
    let (allow_auto, key_ref) = cfg.cfg_of(id).unwrap_or_default();
    // a login kept in the person's vault (Copilot): its token goes to this one run as the variable the program reads
    let key = if key_ref.is_empty() { cx.services.agent_vault_token(&cx.session.user, id).map(|(_, t)| t) } else { Some(cx.services.secrets.resolve(&key_ref, None).map_err(|e| format!("The API key `{key_ref}` could not be read from the vault: {e}. Open Settings → Agents to pick another."))?) };
    let resume = sid.as_ref().and_then(|s| cx.services.ext_sessions.get(s, id));
    let (svc, user) = (cx.services.clone(), cx.session.user.clone());
    let (tools2, audit, user2) = (tools.clone(), cx.audit.clone(), cx.session.user.clone());
    let (svc2, sid2, aid2) = (svc.clone(), sid.clone(), id.to_string());
    Ok(RunRequest {
        agent: agent.clone(),
        workspace,
        prompt: text.to_string(),
        system: Arc::new(move || match &prepare {
            Some(p) => p(&tools2.defs()).iter().filter(|m| m.role == Role::System).map(|m| m.content.clone()).collect::<Vec<_>>().join("\n\n"),
            None => String::new(),
        }),
        model,
        resume,
        tools,
        limits,
        allow_auto,
        key,
        audit: Arc::new(move |k, d| audit.record(&user2, k, d)),
        exe: std::env::current_exe().unwrap_or_default(),
        on_session: Arc::new(move |s| {
            if let Some(sid) = &sid2 {
                svc2.ext_sessions.set(sid, &aid2, s);
            }
        }),
        on_usage: Arc::new(move |l, m, i, o| svc.usage.record(&user, l, m, i, o)),
        no_tools: false,
    })
}

/// The Autopilot steps of a parallel task carried out by an external agent. The agent session is the task's own (`sid` is
/// `task-<id>`, so `--resume` never crosses tasks) and the agent works in the task's tree. An agent that cannot use IYAKKI's
/// tools (Copilot advises only) is refused with the reason.
pub(in crate::apps) fn task_executor(cx: &AppCx, id: &str, tools: Arc<dyn iyakki_agent::agent::Tools>, workspace: std::path::PathBuf, sid: String, model: String) -> iyakki_agent::autopilot::Executor {
    let (cx, id) = (cx.clone(), id.to_string());
    let refusal = cx.services.agents_cfg().agent(&id).filter(|a| a.mode() != extagent::Mode::Bridged).map(|a| format!("{} can only advise here (it cannot use IYAKKI's tools), so it cannot run a task. Use the built-in model or Claude Code for tasks.", a.label()));
    Arc::new(move |call: iyakki_agent::autopilot::StepCall| {
        let made = match &refusal {
            Some(r) => Err(r.clone()),
            None => make_request(&cx, &id, &call.prompt, tools.clone(), call.limits, call.prepare, Some(workspace.clone()), Some(sid.clone()), model.clone()),
        };
        match made {
            Ok(req) => extagent::run::start(req),
            Err(e) => {
                let (tx, rx) = std::sync::mpsc::channel();
                let (dtx, _drx) = std::sync::mpsc::channel();
                let _ = tx.send(RunEvent::Failed { error: e });
                iyakki_agent::agent::RunHandle::from_parts(rx, dtx)
            }
        }
    })
}
