//! One external-agent run: starts the CLI in the workspace, turns its event stream into the chat's [`RunEvent`]s (streaming
//! text, thinking, tool blocks, approvals), keeps the vendor's session id for the next message, records the vendor's own
//! usage numbers, and kills the whole process group on Stop or timeout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use super::bridge::{Audit, Bridge, Correlator, Decisions, Gate};
use super::{classify_failure, tidy, Ev, ExternalAgent, Mode, ParseState, Plan};
use crate::agent::{CallInfo, RunEvent, RunHandle, RunLimits, Tools};

pub struct RunRequest {
    pub agent: Arc<dyn ExternalAgent>,
    pub workspace: PathBuf,
    pub prompt: String,
    /// The PROJECT STATE, skills and rules; built on the run's own thread (it may read the disk and search the history).
    pub system: Arc<dyn Fn() -> String + Send + Sync>,
    pub model: String,
    /// The vendor's session id from the last message of this chat.
    pub resume: Option<String>,
    pub tools: Arc<dyn Tools>,
    pub limits: RunLimits,
    /// This agent may use the approval mode Auto (Settings → Agents).
    pub allow_auto: bool,
    pub key: Option<Zeroizing<String>>,
    pub audit: Audit,
    /// IYAKKI's own program, started by the agent as the MCP server (`--mcp-tools <socket>`).
    pub exe: PathBuf,
    pub on_session: Arc<dyn Fn(&str) + Send + Sync>,
    /// (agent label, model, input tokens, output tokens) as the vendor reported them.
    pub on_usage: Arc<dyn Fn(&str, &str, u64, u64) + Send + Sync>,
    /// Story content only (TALES): every tool request, IYAKKI's or the agent's own, is denied, and the IDE rules are not added.
    pub no_tools: bool,
}

/// The rules every external agent gets on top of the PROJECT STATE and skills (the built-in assistant has the same ones).
pub fn rules(label: &str, mode: Mode) -> String {
    let mut s = format!(
        "\n\nYou are {label}, running inside IYAKKI's AI IDE for the person who is watching this chat.\n\
         - Do the work yourself: edit the files, run the tests, fix what fails. Never ask the person to edit a file or to run a command you can run.\n\
         - Track work on the feature board with the board_* tools (rows are B-<n> for bugs and F-<n> for features): list it first, add or update a row when you start or finish, link tests with board_link_tests. Report problems with bug_report.\n\
         - Before starting, call history_search: earlier chats and Autopilot steps of this workspace are kept there, so you do not start from scratch.\n\
         - The PROJECT STATE starts with the PROJECT CONTEXT, shared by the whole team and every agent (decisions, constraints, gotchas of this project): follow the entries people wrote. When the person states a lasting rule, or you find a trap, record it with mcp__iyakki__context_add; never put a secret in it.\n\
         - Work only inside the workspace folder. Never edit `.iyakki/` yourself (use the board tools).\n\
         - IYAKKI is an operating system for agents with a launcher of tiles and an app Store. When the person asks for \"an IYAKKI app\", \"a tile\" or \"an app for IYAKKI\" they mean a tile app (HTML, CSS and JavaScript with an app.json) published to the Store or their private store, never a Python, Node or Docker web service on the cluster unless they explicitly ask for a cluster deployment.\n"
    );
    if mode == Mode::Bridged {
        s.push_str("- IYAKKI's tools are the `mcp__iyakki__*` tools. Use mcp__iyakki__k8s_*, mcp__iyakki__deploy_* and mcp__iyakki__run_command for the cluster, containers and other commands; kubectl, k3s, kubeadm, docker rm and sudo are refused in the raw shell. Each action may wait for the person to approve it; if one is denied, do not retry it.\n");
        s.push_str(&format!("- {}\n", crate::jobtools::EXTERNAL_RULE));
        s.push_str("- To make and publish an IYAKKI app: tile_scaffold, edit the web files, tile_test, then tile_ship (one step: check, pack, sign, publish; the person sees one approval card). Tile apps are plain HTML, CSS and JavaScript, never Rust, Python, Docker or k3s. Keys and tokens are kept by IYAKKI: never ask the person for them. tile_pack, store_validate, store_publish and tile_install_dev exist for single steps.\n");
        if label.contains("Copilot") {
            // Copilot names an MCP server's tools `<server>-<tool>`
            s = s.replace("mcp__iyakki__", "iyakki-");
            s.push_str("- Your own file-writing, shell and web tools are switched off here. Read with view, grep and glob; create or replace a file with iyakki-write_file (the whole file), run commands with iyakki-run_command, fetch web pages with the iyakki-web_* tools.\n");
        }
    } else {
        s.push_str("- You are running as an advisor: you may read the workspace but not change it. Show the exact edits (as diffs or complete files) in your answer; the person applies them.\n");
    }
    s
}

fn strip(name: &str) -> &str {
    name.strip_prefix(super::claude_code::TOOL_PREFIX).unwrap_or(name)
}

/// (size, modified) of every file of the workspace, to see what an agent that IYAKKI cannot hold to a policy changed.
fn snapshot(root: &Path) -> HashMap<String, (u64, u64)> {
    fn walk(dir: &Path, root: &Path, out: &mut HashMap<String, (u64, u64)>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if out.len() > 20_000 {
                return;
            }
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                if !matches!(name.as_str(), ".git" | "target" | "node_modules" | ".iyakki" | ".venv" | "__pycache__") {
                    walk(&p, root, out);
                }
            } else if md.is_file() {
                let m = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
                out.insert(p.strip_prefix(root).unwrap_or(&p).display().to_string(), (md.len(), m));
            }
        }
    }
    let mut out = HashMap::new();
    walk(root, root, &mut out);
    out
}

fn changed(before: &HashMap<String, (u64, u64)>, after: &HashMap<String, (u64, u64)>) -> Vec<String> {
    let mut v: Vec<String> = after.iter().filter(|(k, a)| before.get(*k) != Some(a)).map(|(k, _)| format!("changed {k}")).chain(before.keys().filter(|k| !after.contains_key(*k)).map(|k| format!("removed {k}"))).collect();
    v.sort();
    v
}

pub fn start(req: RunRequest) -> RunHandle {
    let (etx, erx) = channel();
    let (dtx, drx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::spawn(move || drive(req, etx, drx, c2));
    RunHandle { events: erx, decisions: dtx, cancel, pause: None }
}

fn drive(req: RunRequest, etx: Sender<RunEvent>, drx: std::sync::mpsc::Receiver<(String, crate::agent::Verdict)>, cancel: Arc<AtomicBool>) {
    let t0 = Instant::now();
    let label = req.agent.label();
    let emit = |e: RunEvent| {
        let _ = etx.send(e);
    };
    let decisions = Decisions::new(drx);
    let corr = Arc::new(Correlator::default());
    let auto = req.limits.auto_now() && req.allow_auto;
    if req.limits.auto_now() && !req.allow_auto {
        let mode = req.limits.live.as_ref().map_or("Auto", |l| l.get().label());
        emit(RunEvent::Note { text: format!("{mode} is on, but Auto is off for {label} (not allowed in Settings → Agents), so every action still asks first. Allow it in Settings → Agents → {label} → Allow Auto (key a).") });
    }
    let mut gate = Gate::new(req.tools.clone(), auto, etx.clone(), decisions, corr.clone(), cancel.clone(), req.limits.approval_timeout, &label, req.audit.clone(), &req.workspace, req.limits.max_result_chars);
    gate.grants = req.limits.grants.clone();
    gate.deny_all = req.no_tools;
    if req.agent.id() == super::COPILOT && req.agent.mode() == Mode::Bridged {
        gate.offer_write_file();
    }
    // the chat's live mode: every permission prompt reads it, so a switch during the run affects the next prompt
    gate.live = req.limits.live.clone();
    gate.allow_auto = req.allow_auto;
    gate.auto_refused_said = AtomicBool::new(req.limits.auto_now() && !req.allow_auto);
    let gate = Arc::new(gate);
    let mode = req.agent.mode();
    let bridge = if mode == Mode::Bridged {
        match Bridge::start(gate.clone(), &req.exe) {
            Ok(b) => Some(b),
            Err(e) => {
                emit(RunEvent::Failed { error: format!("Could not open IYAKKI's tool socket for {label}: {e}") });
                return;
            }
        }
    } else {
        None
    };
    let mut system = (req.system)();
    if !req.no_tools {
        system.push_str(&rules(&label, mode));
        if mode == Mode::Bridged {
            system.push_str(&super::guidance::for_prompt(&req.prompt));
        }
    }
    let plan = Plan { workspace: &req.workspace, prompt: &req.prompt, system: &system, model: &req.model, resume: req.resume.as_deref(), mcp_config: bridge.as_ref().map(|b| b.config.as_path()), key: req.key.as_ref().map(|k| k.as_str()), max_turns: req.limits.max_steps.max(1) * 4, no_tools: req.no_tools };
    let inv = req.agent.invocation(&plan);
    (req.audit)("extagent.start", &format!("{label} {} in {} ({})", if req.model.is_empty() { "default model" } else { &req.model }, req.workspace.display(), mode.label()));
    let before = (mode == Mode::Advisor).then(|| snapshot(&req.workspace));
    let mut sp = match super::proc::spawn(&inv.argv, &req.workspace, &inv.env, inv.stdin.as_deref()) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let (_, fix) = super::explain(req.agent.as_ref(), false, &super::Auth::Unknown);
            emit(RunEvent::Failed { error: fix });
            return;
        }
        Err(e) => {
            emit(RunEvent::Failed { error: format!("{label} could not be started: {e}") });
            return;
        }
    };
    let mut st = ParseState::default();
    let mut names: HashMap<String, String> = HashMap::new();
    let mut pending_text = String::new();
    let mut answer = String::new();
    let mut done: Option<Ev> = None;
    let mut model_used = req.model.clone();
    let (mut active, mut last_tick, mut last_line) = (Duration::ZERO, Instant::now(), Instant::now());
    let mut stopped = false;
    let mut tool_end: Option<String> = None;
    let mut timeout: Option<String> = None;
    let mut done_at: Option<Instant> = None;
    let silence = Duration::from_secs(300);
    let think0 = Instant::now();
    loop {
        // time counts only while nobody is being asked
        let now = Instant::now();
        if gate.waiting.load(Ordering::SeqCst) == 0 {
            active += now - last_tick;
        } else {
            last_line = now;
        }
        last_tick = now;
        if cancel.load(Ordering::SeqCst) {
            stopped = true;
            break;
        }
        if let Some(m) = gate.end.lock().unwrap_or_else(|e| e.into_inner()).take() {
            tool_end = Some(m);
            break;
        }
        if active.as_secs() > req.limits.max_seconds {
            timeout = Some(format!("{label} ran longer than the time limit ({} s) and was stopped.", req.limits.max_seconds));
            break;
        }
        if last_line.elapsed() > silence {
            timeout = Some(format!("{label} printed nothing for {} minutes and was stopped. It may be waiting for something it cannot get here (a sign-in, a question).", silence.as_secs() / 60));
            break;
        }
        if done_at.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
            break; // the answer is in; do not wait for a child that holds the pipe open
        }
        let raw = match sp.lines.recv_timeout(Duration::from_millis(100)) {
            Ok(l) => l,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        last_line = Instant::now();
        let line = String::from_utf8_lossy(&raw).into_owned();
        let evs = req.agent.parse_line(&line, &mut st);
        let (mut text, mut calls) = (String::new(), Vec::<CallInfo>::new());
        for ev in evs {
            match ev {
                Ev::Init { session, model } => {
                    if !session.is_empty() {
                        (req.on_session)(&session);
                    }
                    if !model.is_empty() {
                        model_used = model;
                    }
                }
                Ev::Delta(t) => emit(RunEvent::Delta { text: t }),
                Ev::Thinking(t) => emit(RunEvent::Thinking { text: t, secs: think0.elapsed().as_secs() }),
                Ev::Text(t) => {
                    text.push_str(&t);
                }
                Ev::ToolUse { id, name, input } => {
                    let shown = strip(&name).to_string();
                    corr.note(&shown, &id);
                    names.insert(id.clone(), shown.clone());
                    calls.push(CallInfo { id, name: shown, args: input });
                }
                Ev::ToolResult { id, ok, text } => {
                    let name = names.get(&id).cloned().unwrap_or_else(|| "tool".into());
                    emit(RunEvent::ToolResult { id, name, ok, text: crate::agent::truncate(&text, req.limits.max_result_chars.max(1000)) });
                }
                Ev::Note(t) => emit(RunEvent::Note { text: t }),
                d @ Ev::Done { .. } => {
                    done_at = Some(Instant::now());
                    done = Some(d);
                }
            }
        }
        if !text.is_empty() || !calls.is_empty() {
            pending_text.push_str(&text);
            if calls.is_empty() {
                answer = pending_text.clone();
            } else {
                emit(RunEvent::Model { text: std::mem::take(&mut pending_text), calls });
                answer.clear();
            }
        }
        if req.agent.plain_text() {
            answer = st.partial.clone();
        }
    }
    // the end of the process: the whole group, then the reap
    if stopped || timeout.is_some() || tool_end.is_some() {
        super::proc::kill_group(&mut sp.child, Duration::from_secs(2));
    }
    let status = if done.is_some() || stopped || timeout.is_some() || tool_end.is_some() {
        super::proc::kill_group(&mut sp.child, Duration::from_millis(300));
        Some(iyakki_core_api::child_status(&mut sp.child)).filter(|s| s.is_over())
    } else {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            match iyakki_core_api::child_status(&mut sp.child) {
                s if s.is_over() => break Some(s),
                _ if Instant::now() > end => {
                    super::proc::kill_group(&mut sp.child, Duration::from_millis(300));
                    break None;
                }
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    };
    // leftovers of the group (a dev server the agent started) do not outlive the run
    super::proc::kill_group(&mut sp.child, Duration::from_millis(100));
    drop(bridge);
    // the process has exited, but its error output may still be in the pipe: take it all (a grandchild that kept the pipe open
    // cannot hold the run for more than a moment)
    let _ = sp.stderr_done.recv_timeout(Duration::from_secs(2));
    let err_text = super::proc::stderr_text(&sp.stderr);
    if !err_text.trim().is_empty() {
        emit(RunEvent::Note { text: format!("{label} error output:\n{}", tidy(err_text.trim(), 1500)) });
    }
    if let Some(before) = before {
        let ch = changed(&before, &snapshot(&req.workspace));
        if !ch.is_empty() {
            emit(RunEvent::Note { text: format!("{label} changed files in the workspace without IYAKKI's approval (IYAKKI cannot hold this agent to its rules):\n{}", ch.iter().take(30).cloned().collect::<Vec<_>>().join("\n")) });
        }
    }
    let secs = t0.elapsed().as_secs();
    if stopped {
        emit(RunEvent::Stopped { reason: "stopped by the user".into() });
        return;
    }
    if let Some(m) = timeout {
        emit(RunEvent::Failed { error: m });
        return;
    }
    if let Some(m) = tool_end {
        // a tool ended the step (the same rule the built-in loop has): its text is the answer
        emit(RunEvent::Final { text: m });
        return;
    }
    let login = req.agent.login_help();
    match done {
        Some(Ev::Done { ok, text, session, cost_usd, input, output, cached, error }) => {
            if let Some(s) = session {
                (req.on_session)(&s);
            }
            if input + output > 0 {
                (req.on_usage)(&label, if model_used.is_empty() { "default" } else { &model_used }, input, output);
            }
            // new tokens only (cache reads are not counted again), like the budgets of the built-in loop
            emit(RunEvent::Usage { tokens: (input - cached.min(input)) + output, cost_cents: cost_usd.map_or(0.0, |c| c * 100.0), files: 0, seconds: secs });
            if !ok {
                emit(RunEvent::Failed { error: classify_failure(&label, &login, &format!("{} {err_text}", error.unwrap_or_default())) });
                return;
            }
            let final_text = if text.trim().is_empty() { answer.clone() } else { text };
            if final_text.trim().is_empty() {
                emit(RunEvent::Failed { error: format!("{label} finished without an answer. Try again, or ask it more specifically.") });
            } else {
                let names: Vec<String> = req.tools.defs().into_iter().map(|d| d.spec.name).collect();
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                if mode == Mode::Bridged && crate::assistant_guard::asks_person_to_act(&final_text, &names) {
                    emit(RunEvent::Note { text: format!("{label} asked you to do something it can do itself here (edit files, run commands through IYAKKI's tools). Answer \"do it yourself\" and it will; in Autopilot this counts as a step that changed nothing.") });
                }
                emit(RunEvent::Final { text: final_text });
            }
        }
        _ => {
            let code = status.and_then(|s| s.code());
            // `Gone`: another reaper took the exit status, so the code is unknown; an answer that came through is taken as the result
            let gone = status == Some(iyakki_core_api::ChildState::Gone);
            if code == Some(0) || ((gone || req.agent.plain_text() && code.is_none()) && !answer.trim().is_empty()) {
                if answer.trim().is_empty() && pending_text.trim().is_empty() {
                    emit(RunEvent::Failed { error: format!("{label} finished without an answer. {}", if err_text.trim().is_empty() { String::new() } else { classify_failure(&label, &login, &err_text) }) });
                } else {
                    emit(RunEvent::Final { text: if answer.trim().is_empty() { pending_text } else { answer } });
                }
            } else {
                let both = format!("{err_text}\n{}", st.partial);
                let why = if both.trim().is_empty() { format!("{label} exited{} without saying why.", code.map_or(String::new(), |c| format!(" with code {c}"))) } else { classify_failure(&label, &login, &both) };
                emit(RunEvent::Failed { error: why });
            }
        }
    }
}

/// Keep a vendor session id per chat and agent, so the next message resumes the same conversation.
pub struct SessionMap {
    path: PathBuf,
    lock: Mutex<()>,
}

impl SessionMap {
    pub fn open(path: &Path) -> SessionMap {
        SessionMap { path: path.into(), lock: Mutex::new(()) }
    }
    fn load(&self) -> serde_json::Map<String, serde_json::Value> {
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    pub fn get(&self, chat: &str, agent: &str) -> Option<String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load().get(&format!("{chat}|{agent}")).and_then(|v| v.as_str()).map(str::to_string)
    }
    pub fn set(&self, chat: &str, agent: &str, id: &str) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = self.load();
        m.insert(format!("{chat}|{agent}"), serde_json::Value::String(id.into()));
        if let Some(p) = self.path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let _ = std::fs::write(&self.path, serde_json::Value::Object(m).to_string());
    }
    pub fn forget_chat(&self, chat: &str) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = self.load();
        m.retain(|k, _| !k.starts_with(&format!("{chat}|")));
        let _ = std::fs::write(&self.path, serde_json::Value::Object(m).to_string());
    }
}
