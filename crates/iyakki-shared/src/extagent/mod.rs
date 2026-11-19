//! External coding agents (Claude Code, GitHub Copilot, a custom command): what one agent is, the settings that build it, the status
//! cache. Moved out of `iyakki_agent::extagent` (F-SPLIT-03); the run thread, the policy bridge, the login and the relay stay in the agent.

pub mod claude_code;
pub mod copilot;
pub mod generic;

/// The Copilot flavour of the run's MCP config, next to the `--mcp-config` file (the agent's `bridge::copilot_config` is the same rule).
pub fn copilot_config(config: &std::path::Path) -> std::path::PathBuf {
    config.with_file_name("mcp-copilot.json")
}

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::wsexec::{Out, Runner};

pub use iyakki_extagentx::*;

// ------------------------------------------------------------------------------------------------ settings

pub use crate::extagentcfg::AgentsCfg;

impl AgentsCfg {
    /// Build the agent for an id (`claude-code`, `copilot`, `custom:<id>`).
    pub fn agent(&self, id: &str) -> Option<Box<dyn ExternalAgent>> {
        match id {
            CLAUDE_CODE => Some(Box::new(claude_code::ClaudeCode::new(self.with_bin(&self.claude, "claude")))),
            COPILOT => Some(Box::new(copilot::Copilot::new(self.with_bin(&self.copilot, "copilot")))),
            other => other.strip_prefix("custom:").and_then(|c| self.custom.iter().find(|x| x.id == c)).map(|c| Box::new(generic::Generic::new(self.custom_with_bin(c))) as Box<dyn ExternalAgent>),
        }
    }

    /// Every agent, in the order the model list shows them.
    pub fn all(&self) -> Vec<Box<dyn ExternalAgent>> {
        let mut v: Vec<Box<dyn ExternalAgent>> = vec![Box::new(claude_code::ClaudeCode::new(self.with_bin(&self.claude, "claude"))), Box::new(copilot::Copilot::new(self.with_bin(&self.copilot, "copilot")))];
        v.extend(self.custom.iter().map(|c| Box::new(generic::Generic::new(self.custom_with_bin(c))) as Box<dyn ExternalAgent>));
        v
    }
}

// ------------------------------------------------------------------------------------------------ the agent

/// What a line of the agent's output means.
#[derive(Clone, Debug, PartialEq)]
pub enum Ev {
    /// The vendor's session id and the model in use.
    Init { session: String, model: String },
    /// Text so far of the answer being written.
    Delta(String),
    /// A finished piece of assistant text.
    Text(String),
    Thinking(String),
    ToolUse { id: String, name: String, input: Value },
    ToolResult { id: String, ok: bool, text: String },
    /// A line for the run log shown as a step (a retry, a warning).
    Note(String),
    /// The end: the vendor's own numbers.
    Done { ok: bool, text: String, session: Option<String>, cost_usd: Option<f64>, input: u64, output: u64, cached: u64, error: Option<String> },
}

pub trait ExternalAgent: Send + Sync {
    /// `claude-code`, `copilot`, `custom:<id>`.
    fn id(&self) -> String;
    fn label(&self) -> String;
    /// The program to start.
    fn program(&self) -> String;
    /// A program named in a command of the person's (`models_command`): by default itself.
    fn program_for(&self, name: &str) -> String {
        name.to_string()
    }
    fn cfg_key_ref(&self) -> String;
    fn cfg_model(&self) -> String;
    /// The documented models shipped with IYAKKI (verified on [`BUILTIN_VERIFIED`]); the first is the agent's own default.
    fn builtin_models(&self) -> Vec<ModelInfo>;
    /// Arguments that make the CLI print its models, when it can do that without a session (`None`: it cannot).
    fn list_models_argv(&self) -> Option<Vec<String>> {
        None
    }
    fn parse_models(&self, o: &Out) -> Vec<ModelInfo> {
        parse_model_lines(&o.stdout)
    }
    /// Models the person typed for this agent.
    fn custom_models(&self) -> Vec<String>;
    fn mode(&self) -> Mode;
    fn version_args(&self) -> Vec<String> {
        vec!["--version".into()]
    }
    /// A command that tells whether the CLI is signed in (its answer goes to `parse_auth`).
    fn auth_args(&self) -> Option<Vec<String>> {
        None
    }
    fn parse_auth(&self, _o: &Out) -> Auth {
        Auth::Unknown
    }
    /// For a CLI with no status command: what its own files say (read-only; never a token).
    fn auth_offline(&self) -> Auth {
        Auth::Unknown
    }
    /// The files the program keeps its login in (paths only, for the log line that says why it looks signed out).
    fn auth_files(&self, _env: &[(String, String)]) -> Vec<std::path::PathBuf> {
        vec![]
    }
    /// The environment this program is looked at and started with besides the process's own: the person's home.
    fn child_env(&self) -> Vec<(String, String)> {
        vec![]
    }
    /// The environment variable an API key is passed in.
    fn key_env(&self) -> Option<String> {
        None
    }
    fn install(&self) -> Option<Install>;
    /// The vendor's own login command, run by the person in the terminal (IYAKKI never types or sees a password).
    fn login(&self) -> Option<Vec<String>>;
    fn login_help(&self) -> String;
    fn invocation(&self, p: &Plan) -> Invocation;
    /// Read one line of output. The state carries what the format needs between lines.
    fn parse_line(&self, line: &str, st: &mut ParseState) -> Vec<Ev>;
    /// Agents whose output is plain text: everything printed is the answer.
    fn plain_text(&self) -> bool {
        false
    }
}

#[derive(Default)]
pub struct ParseState {
    /// Text deltas of the message being written.
    pub partial: String,
    pub thinking: String,
    pub seen_result: bool,
    /// tool_use ids already announced (a partial stream and the full message both carry them).
    pub announced: std::collections::HashSet<String>,
    pub bad_lines: u32,
}

/// What the person sees when the agent is not ready.
pub fn explain(a: &dyn ExternalAgent, installed: bool, auth: &Auth) -> (String, String) {
    if !installed {
        let how = match a.install() {
            Some(i) => format!(" Install it with `{}` (Settings → Agents → Install shows the command and asks first){}.", i.argv.join(" "), if i.needs.is_empty() { String::new() } else { format!("; it needs {} first", i.needs) }),
            None => String::new(),
        };
        return ("not installed".into(), format!("{} is not installed (no `{}` program was found).{how}", a.label(), a.program()));
    }
    if *auth == Auth::SignedOut {
        return ("sign in".into(), a.login_help());
    }
    ("ready".into(), String::new())
}

/// Run a lookup; a program that has not answered in time (a big binary starting cold from the disk right after boot) is given one
/// more, longer try before anything is concluded from the silence.
fn run_patient(runner: &dyn Runner, cwd: &Path, argv: &[String], env: &[(String, String)]) -> Out {
    let o = runner.run(cwd, argv, env, Duration::from_secs(15), 16 * 1024, None);
    if o.timed_out {
        return runner.run(cwd, argv, env, Duration::from_secs(90), 16 * 1024, None);
    }
    o
}

/// Anything that looks like a token or an address replaced, so a line may go to the system log: long runs of token characters,
/// known token prefixes and words with an `@`.
pub fn redact(s: &str) -> String {
    let is_tok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '=' | '+' | '/' | '@');
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if !run.is_empty() {
            let r = run.as_str();
            let prefixed = ["sk-", "ghp_", "gho_", "ghu_", "ghs_", "github_pat_", "eyJ", "xox"].iter().any(|p| r.contains(p)) && r.len() >= 12;
            if r.contains('@') && r.len() > 3 && !r.starts_with('@') {
                out.push_str("[address]");
            } else if prefixed || (r.len() >= 24 && !r.starts_with('/')) {
                out.push_str("[redacted]");
            } else {
                out.push_str(r);
            }
            run.clear();
        }
    };
    for c in s.chars() {
        if is_tok(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn home_of(env: &[(String, String)], k: &str) -> String {
    env.iter().find(|(n, _)| n == k).map_or_else(|| format!("(process: {})", std::env::var(k).unwrap_or_default()), |(_, v)| v.clone())
}

/// One line for the system log saying why an agent looks signed out: the home and config folder it was asked in, the exit code,
/// whether its login file is there (the path only) and the start of what it printed, with anything token-like replaced.
pub fn signed_out_line(a: &dyn ExternalAgent, env: &[(String, String)], o: Option<&Out>) -> String {
    let files: Vec<String> = a.auth_files(env).iter().map(|f| format!("{} {}", f.display(), if f.is_file() { "exists" } else { "missing" })).collect();
    let (code, text) = match o {
        Some(o) => (o.code.map_or("none".to_string(), |c| c.to_string()) + if o.timed_out { " (timed out)" } else { "" }, redact(&tidy(&o.combined().replace('\n', " "), 160))),
        None => ("-".to_string(), String::new()),
    };
    format!("agent {}: looks signed out (HOME={}, CLAUDE_CONFIG_DIR={}, exit {code}, login files: [{}], output: {text})", a.id(), home_of(env, "HOME"), home_of(env, "CLAUDE_CONFIG_DIR"), files.join(", "))
}

/// Look at the program: is it there, which version, is it signed in. Only `--version` and the status command run; no model is called.
pub fn detect(a: &dyn ExternalAgent, runner: &dyn Runner, cwd: &Path) -> Status {
    let prog = a.program();
    let env = a.child_env();
    let mut argv = vec![prog.clone()];
    argv.extend(a.version_args());
    let o = run_patient(runner, cwd, &argv, &env);
    if o.missing || (o.code.is_none() && !o.timed_out) {
        let (chip, fix) = explain(a, false, &Auth::Unknown);
        return Status { installed: false, version: String::new(), auth: Auth::Unknown, mode: a.mode(), chip, fix };
    }
    let version = o.combined().lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().chars().take(80).collect::<String>();
    let installed = o.ok();
    if !installed {
        let (chip, fix) = explain(a, false, &Auth::Unknown);
        return Status { installed: false, version, auth: Auth::Unknown, mode: a.mode(), chip, fix: format!("{fix} (`{} --version` failed: {})", prog, o.combined().lines().next().unwrap_or("")) };
    }
    let mut last: Option<Out> = None;
    let auth = if !a.cfg_key_ref().is_empty() {
        Auth::Key
    } else {
        match a.auth_args() {
            Some(args) => {
                let mut argv = vec![prog];
                argv.extend(args);
                let o = run_patient(runner, cwd, &argv, &env);
                let r = a.parse_auth(&o);
                last = Some(o);
                r
            }
            None => a.auth_offline(),
        }
    };
    if auth == Auth::SignedOut {
        log::warn!("{}", signed_out_line(a, &env, last.as_ref()));
    }
    let (chip, mut fix) = explain(a, true, &auth);
    // the login files live in the home folder on the boot drive: while it is lost "signed out" is not the real reason
    let snap = iyakki_storage::loss::snapshot();
    if !snap.state.healthy() {
        fix = format!("{} {fix}", iyakki_storage::loss::unavailable_text(&snap));
    }
    Status { installed, version, auth, mode: a.mode(), chip, fix }
}

/// Plain words for a failed run, with the fix, from what the program printed.
pub fn classify_failure(label: &str, login_help: &str, text: &str) -> String {
    // the program's home (login, settings, work folder) is on the boot drive: while that is lost, say so instead of a bare error
    let snap = iyakki_storage::loss::snapshot();
    if !snap.state.healthy() {
        return format!("{label} cannot work right now. {}", iyakki_storage::loss::unavailable_text(&snap));
    }
    let l = text.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| l.contains(w));
    let tail = tidy(text.trim(), 400);
    if has(&["not logged in", "please run /login", "invalid api key", "authentication_failed", "authentication failed", "unauthorized", "401", "no token", "not authenticated", "sign in"]) {
        return format!("{label} is not signed in. {login_help}");
    }
    if has(&["rate limit", "rate_limit", "429", "overloaded", "quota", "usage limit", "too many requests"]) {
        return format!("{label} is rate limited or out of quota right now. Wait a little and try again, or pick another model. ({tail})");
    }
    if has(&["enotfound", "econnrefused", "econnreset", "etimedout", "network", "fetch failed", "could not resolve", "unable to connect", "no route"]) {
        return format!("{label} could not reach its service: the network seems down. Check the connection and try again. ({tail})");
    }
    format!("{label} stopped with an error: {tail}")
}

// ------------------------------------------------------------------------------------------------ status cache and the model list

/// What the last look at each agent found. Detection starts programs, so it runs on a thread of its own and the screens read this.
#[derive(Default)]
pub struct StatusCache {
    map: std::sync::Mutex<std::collections::BTreeMap<String, Status>>,
    busy: std::sync::atomic::AtomicBool,
    /// Bumped when the person signs in or out: a lookup that began before it describes another home and is dropped.
    generation: std::sync::atomic::AtomicU64,
    /// A lookup asked for while one was running: done right after it, with the newest settings.
    pending: std::sync::Mutex<Option<AgentsCfg>>,
    /// When each agent was last looked at.
    looked: std::sync::Mutex<std::collections::BTreeMap<String, std::time::Instant>>,
    /// Model lists the CLIs printed: (when, models). Kept for [`MODELS_TTL`] and saved in `file`.
    models: std::sync::Mutex<std::collections::BTreeMap<String, (u64, Vec<ModelInfo>)>>,
    file: Option<std::path::PathBuf>,
}

/// A model list a CLI printed is trusted for a day.
pub const MODELS_TTL: u64 = 24 * 3600;

pub fn now_secs() -> u64 {
    iyakki_clock::now_unix().max(0) as u64
}

impl StatusCache {
    /// A cache that keeps the models the CLIs printed in `file` (the agents cache).
    pub fn open(file: &Path) -> StatusCache {
        let models = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        StatusCache { models: std::sync::Mutex::new(models), file: Some(file.into()), ..Default::default() }
    }

    /// Remember a list a CLI printed (now).
    pub fn set_models(&self, id: &str, list: Vec<ModelInfo>) {
        self.set_models_at(id, list, now_secs());
    }

    pub fn set_models_at(&self, id: &str, list: Vec<ModelInfo>, at: u64) {
        let mut g = self.models.lock().unwrap_or_else(|e| e.into_inner());
        g.insert(id.into(), (at, list));
        if let Some(f) = &self.file {
            let _ = std::fs::write(f, serde_json::to_string(&*g).unwrap_or_default());
        }
    }

    /// The list a CLI printed, while it is fresh.
    pub fn cli_models(&self, id: &str) -> Option<Vec<ModelInfo>> {
        let g = self.models.lock().unwrap_or_else(|e| e.into_inner());
        g.get(id).filter(|(at, l)| now_secs().saturating_sub(*at) < MODELS_TTL && !l.is_empty()).map(|(_, l)| l.clone())
    }

    /// The models to offer for an agent: what the CLI printed (fresh), else the documented list; then the ids the person typed and
    /// the one currently chosen if it is none of those.
    pub fn models_for(&self, a: &dyn ExternalAgent) -> Vec<ModelInfo> {
        let mut v = self.cli_models(&a.id()).unwrap_or_else(|| a.builtin_models());
        let mut extra: Vec<String> = a.custom_models();
        let cur = a.cfg_model();
        if !cur.is_empty() {
            extra.push(cur);
        }
        for id in extra {
            if valid_model_id(&id) && !v.iter().any(|m| m.id == id) {
                v.push(ModelInfo::new(&id, &id, "", ModelSource::Custom));
            }
        }
        v
    }

    pub fn get(&self, id: &str) -> Option<Status> {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
    }
    pub fn set(&self, id: &str, s: Status) {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).insert(id.into(), s);
        self.looked.lock().unwrap_or_else(|e| e.into_inner()).insert(id.into(), std::time::Instant::now());
    }
    pub fn busy(&self) -> bool {
        self.busy.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Forget what was found (the person signed in or out, so the home the programs look in changed): the screens show
    /// "checking…" and look again with the new settings; a lookup still running for the old home is dropped.
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.map.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.looked.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// [`invalidate`](Self::invalidate), then look at every agent again with `cfg`.
    pub fn reset(self: &std::sync::Arc<Self>, cfg: AgentsCfg) {
        self.invalidate();
        self.refresh(cfg);
    }

    /// A screen shows an agent that looks signed out (or not found): look again, at most every 5 seconds, so a status found at a
    /// bad moment (before sign-in, with the network down, the program starting cold) mends itself.
    pub fn recheck(self: &std::sync::Arc<Self>, cfg: AgentsCfg, id: &str) {
        self.recheck_after(cfg, id, Duration::from_secs(5));
    }

    pub fn recheck_after(self: &std::sync::Arc<Self>, cfg: AgentsCfg, id: &str, min: Duration) {
        let stale = match self.get(id) {
            Some(s) => !s.ready() || !s.installed,
            None => return,
        };
        let due = self.looked.lock().unwrap_or_else(|e| e.into_inner()).get(id).is_none_or(|t| t.elapsed() >= min);
        if stale && due {
            self.looked.lock().unwrap_or_else(|e| e.into_inner()).insert(id.into(), std::time::Instant::now());
            self.refresh_one(cfg, id, None);
        }
    }

    /// Look at one agent again (after a sign-in or sign-out), on a thread of its own; `done` is called with the new status.
    pub fn refresh_one(self: &std::sync::Arc<Self>, cfg: AgentsCfg, id: &str, done: Option<Box<dyn FnOnce(&Status) + Send>>) {
        let me = self.clone();
        let id = id.to_string();
        std::thread::spawn(move || {
            if let Some(a) = cfg.agent(&id) {
                let gen = me.generation.load(std::sync::atomic::Ordering::SeqCst);
                let st = detect(a.as_ref(), &crate::wsexec::System, &std::env::temp_dir());
                if me.generation.load(std::sync::atomic::Ordering::SeqCst) == gen {
                    me.set(&id, st.clone());
                }
                if let Some(f) = done {
                    f(&st);
                }
            }
        });
    }

    /// Look at every agent again, on a thread of its own (`--version` and the status command only; no model is called). A request
    /// that comes while one is running is not lost: it runs next, with the newest settings.
    pub fn refresh(self: &std::sync::Arc<Self>, cfg: AgentsCfg) {
        {
            let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            if self.busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
                *p = Some(cfg);
                return;
            }
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let mut cfg = cfg;
            loop {
                me.refresh_now(&cfg);
                let mut p = me.pending.lock().unwrap_or_else(|e| e.into_inner());
                match p.take() {
                    Some(next) => cfg = next,
                    None => {
                        me.busy.store(false, std::sync::atomic::Ordering::SeqCst);
                        break;
                    }
                }
            }
        });
    }

    fn refresh_now(&self, cfg: &AgentsCfg) {
        let cwd = std::env::temp_dir();
        let gen = self.generation.load(std::sync::atomic::Ordering::SeqCst);
        for a in cfg.all() {
            let st = detect(a.as_ref(), &crate::wsexec::System, &cwd);
            // only when the person pressed Detect and the program is there: ask the CLI for its models, with a short timeout
            if st.installed {
                if let Some(mut argv) = a.list_models_argv() {
                    if !argv[0].contains('/') {
                        argv[0] = a.program_for(&argv[0]);
                    }
                    let o = crate::wsexec::System.run(&cwd, &argv, &[], Duration::from_secs(10), 64 * 1024, None);
                    let list = if o.ok() { a.parse_models(&o) } else { vec![] };
                    if !list.is_empty() {
                        self.set_models(&a.id(), list);
                    }
                }
            }
            if self.generation.load(std::sync::atomic::Ordering::SeqCst) != gen {
                return; // signed in or out meanwhile: this describes the old home
            }
            self.set(&a.id(), st);
        }
    }
}

/// Every agent as a line of the model list, with the status found so far (unavailable ones are listed too).
pub fn entries(cfg: &AgentsCfg, cache: &StatusCache) -> Vec<Entry> {
    cfg.all()
        .into_iter()
        .filter(|a| a.id().starts_with("custom:") || if a.id() == CLAUDE_CODE { cfg.claude.enabled } else { cfg.copilot.enabled })
        .map(|a| {
            let st = cache.get(&a.id());
            let model = if a.cfg_model().is_empty() { "default".to_string() } else { a.cfg_model() };
            let (chip, ready, fix) = match &st {
                Some(s) => (s.chip.clone(), s.ready(), s.fix.clone()),
                None => ("checking…".to_string(), true, String::new()),
            };
            let label = format!("{} (agent)", a.label());
            Entry { provider: provider_of(&a.id()), id: a.id(), model, label, chip, ready, fix }
        })
        .collect()
}

pub use crate::agent::NoTools;
