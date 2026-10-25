//! External coding agents (Claude Code, GitHub Copilot, a custom command) that run next to the built-in models.
//! IYAKKI only drives command-line programs the person installed and signed into; it never handles a vendor password.
//! See `docs/EXTERNAL-AGENTS.md` for the verified command-line interfaces and what could not be verified.
//!
//! * [`ExternalAgent`]: what one agent is (how to detect it, how to start it, how to read its output).
//! * [`run`]: the run thread: starts the CLI, turns its event stream into the chat's `RunEvent`s, kills it on Stop.
//! * [`bridge`]: the policy bridge: IYAKKI's tools and approvals offered to the agent over a local MCP socket.
//! * [`proc`]: the one place a CLI is started and killed.

pub use zeroize::Zeroizing;

pub mod bridge;
pub mod claude_code;
pub mod copilot;
pub mod generic;
pub mod guidance;
pub mod login;
pub mod relay;
pub mod proc;
pub mod run;
#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod autopilot_tests;
#[cfg(test)]
mod auth_tests;

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::wsexec::{Out, Runner};

/// The id of the built-in entries; a custom agent is `custom:<id>`.
pub const CLAUDE_CODE: &str = "claude-code";
pub const COPILOT: &str = "copilot";
/// Provider names in the model list: `agent:<id>`.
pub const PROVIDER_PREFIX: &str = "agent:";

/// The model-list provider for an agent id.
pub fn provider_of(id: &str) -> String {
    format!("{PROVIDER_PREFIX}{id}")
}

/// The agent id in a model-list provider (`None` for an ordinary provider).
pub fn agent_of(provider: &str) -> Option<&str> {
    provider.strip_prefix(PROVIDER_PREFIX)
}

// ------------------------------------------------------------------------------------------------ settings

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentCfg {
    pub enabled: bool,
    /// The program (a name on PATH or an absolute path); empty = the vendor's usual name.
    pub command: String,
    /// The model the agent is asked to use; empty = the agent's own default.
    pub model: String,
    /// Model ids the person typed ("Custom model id…"), remembered for this agent.
    pub custom_models: Vec<String>,
    /// The vault entry holding an API key (`vault:NAME` reference), passed to the program as an environment variable only.
    pub key_ref: String,
    /// May the approval mode Auto answer this agent's prompts? Off until the person turns it on.
    pub allow_auto: bool,
    /// Copilot only: run it as a read-only advisor instead of offering IYAKKI's tools over MCP (the default is the policy bridge,
    /// verified against a live CLI; see docs/EXTERNAL-AGENTS.md).
    pub advisor_only: bool,
    /// The person's home for the program (`HOME`, and `CLAUDE_CONFIG_DIR` for Claude Code): its login lives there. Filled in by
    /// `Services::agents_cfg`, never saved. Empty: the process's own.
    #[serde(skip)]
    pub env: Vec<(String, String)>,
    /// Who the token kept in the person's vault belongs to (never the token itself); filled in, never saved.
    #[serde(skip)]
    pub vault_login: Option<String>,
    /// The person's vault is locked, so a token in it cannot be seen; filled in, never saved.
    #[serde(skip)]
    pub vault_locked: bool,
}

impl Default for AgentCfg {
    fn default() -> Self {
        AgentCfg { enabled: true, command: String::new(), model: String::new(), custom_models: vec![], key_ref: String::new(), allow_auto: false, advisor_only: false, env: vec![], vault_login: None, vault_locked: false }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomCfg {
    /// Letters, digits and dashes.
    pub id: String,
    pub name: String,
    /// The command line, split into words without a shell. `{prompt}` in a word is replaced by the request; without it the
    /// request goes to standard input.
    pub command: String,
    pub model: String,
    /// A command that prints the models it offers, one per line (`id`, then optionally a note); run only when the person presses Detect.
    pub models_command: String,
    pub custom_models: Vec<String>,
    pub key_ref: String,
    /// The environment variable the key is passed in (for example `OPENAI_API_KEY`).
    pub key_env: String,
    pub allow_auto: bool,
}

impl Default for CustomCfg {
    fn default() -> Self {
        CustomCfg { id: String::new(), name: String::new(), command: String::new(), model: String::new(), models_command: String::new(), custom_models: vec![], key_ref: String::new(), key_env: String::new(), allow_auto: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsCfg {
    pub claude: AgentCfg,
    pub copilot: AgentCfg,
    pub custom: Vec<CustomCfg>,
    /// The agent new chats start with (`None`: a built-in model).
    pub default: Option<String>,
    /// The CLI tile's folder of installed programs: a program installed there is found without a command being set
    /// (filled in by `Services::agents_cfg`, never saved).
    #[serde(skip)]
    pub extra_bin: Option<std::path::PathBuf>,
    /// The signed-in person's home for the vendor programs (`HOME`, `CLAUDE_CONFIG_DIR`), the same one the login uses (filled in by
    /// `Services::agents_cfg`, never saved).
    #[serde(skip)]
    pub env: Vec<(String, String)>,
    /// Agent id -> the account whose token is in the person's vault.
    #[serde(skip)]
    pub vault_logins: std::collections::BTreeMap<String, String>,
    #[serde(skip)]
    pub vault_locked: bool,
}

pub fn valid_custom_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 24 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !s.starts_with('-')
}

/// A model id typed by the person: it ends up as one argument of the agent's command line, so only the characters model ids
/// use (letters, digits and `. _ - : / @ + [ ]`) are accepted, never a space, quote, `$`, `;`, `&`, `|`, `<`, `>` or a leading dash.
pub fn valid_model_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 80 && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/' | '@' | '+' | '[' | ']'))
}

/// Where a model line of the picker comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelSource {
    /// The CLI printed it (`models_command`).
    Cli,
    /// The documented aliases shipped with IYAKKI (`BUILTIN_VERIFIED`).
    BuiltIn,
    /// Typed by the person.
    Custom,
}

impl ModelSource {
    pub fn label(self) -> &'static str {
        match self {
            ModelSource::Cli => "from the CLI",
            ModelSource::BuiltIn => "built-in list",
            ModelSource::Custom => "your own id",
        }
    }
}

/// One selectable model of an agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// What `--model` gets; empty = the agent chooses (no flag).
    pub id: String,
    pub label: String,
    /// "fast, cheap", "most capable"… where the vendor says so.
    pub note: String,
    pub source: ModelSource,
    /// `Some(false)` only when the CLI itself said it is not available to the signed-in account (never guessed).
    pub available: Option<bool>,
}

impl ModelInfo {
    pub fn new(id: &str, label: &str, note: &str, source: ModelSource) -> ModelInfo {
        ModelInfo { id: id.into(), label: label.into(), note: note.into(), source, available: None }
    }
}

/// When the documented lists were checked against the vendors' pages.
pub const BUILTIN_VERIFIED: &str = "2026-10-16";

/// Read a model list a program printed: one model per line, `id` then an optional note after a tab, ` - ` or two spaces.
/// A note that says the model is unavailable or needs something marks it so; nothing else is assumed.
pub fn parse_model_lines(text: &str) -> Vec<ModelInfo> {
    let mut out: Vec<ModelInfo> = vec![];
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (id, note) = match line.find(['\t']).or_else(|| line.find("  ")).or_else(|| line.find(" - ")) {
            Some(i) => (line[..i].trim(), line[i..].trim().trim_start_matches('-').trim()),
            None => (line, ""),
        };
        if !valid_model_id(id) || out.iter().any(|m| m.id == id) {
            continue;
        }
        let low = note.to_lowercase();
        let unavailable = ["unavailable", "not available", "requires", "no access"].iter().any(|w| low.contains(w));
        let mut m = ModelInfo::new(id, id, &tidy(note, 80), ModelSource::Cli);
        if unavailable {
            m.available = Some(false);
        }
        out.push(m);
        if out.len() >= 60 {
            break;
        }
    }
    out
}

pub fn valid_env_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') && !s.chars().next().is_some_and(|c| c.is_ascii_digit())
}

impl AgentsCfg {
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for c in &self.custom {
            if !valid_custom_id(&c.id) {
                return Err(format!("custom agent `{}`: the id may use letters, digits and dashes", c.id));
            }
            if !seen.insert(c.id.clone()) {
                return Err(format!("duplicate custom agent id `{}`", c.id));
            }
            if c.command.trim().is_empty() {
                return Err(format!("custom agent `{}`: give the command to run", c.id));
            }
            if c.model.contains(' ') || c.custom_models.iter().any(|m| !valid_model_id(m)) || (!c.model.is_empty() && !valid_model_id(&c.model)) {
                return Err(format!("custom agent `{}`: a model id has no spaces or shell characters", c.id));
            }
            if !c.key_env.is_empty() && !valid_env_name(&c.key_env) {
                return Err(format!("custom agent `{}`: the key variable must look like OPENAI_API_KEY", c.id));
            }
        }
        for (n, a) in [("Claude Code", &self.claude), ("GitHub Copilot", &self.copilot)] {
            if (!a.model.is_empty() && !valid_model_id(&a.model)) || a.custom_models.iter().any(|m| !valid_model_id(m)) {
                return Err(format!("{n}: a model id has no spaces or shell characters"));
            }
        }
        for (n, k) in [("Claude Code", &self.claude.key_ref), ("GitHub Copilot", &self.copilot.key_ref)] {
            if !k.is_empty() && !k.starts_with("vault:") {
                return Err(format!("{n}: the API key must come from the vault (vault:NAME)"));
            }
        }
        Ok(())
    }

    /// A program installed from the CLI tile is used when no command is set.
    fn with_bin(&self, c: &AgentCfg, name: &str) -> AgentCfg {
        let mut c = c.clone();
        if c.command.trim().is_empty() {
            if let Some(p) = self.extra_bin.as_ref().map(|d| d.join(name)).filter(|p| p.exists()) {
                c.command = p.display().to_string();
            }
        }
        c.env = self.env.clone();
        c.vault_login = self.vault_logins.get(if name == "claude" { CLAUDE_CODE } else { COPILOT }).cloned();
        c.vault_locked = self.vault_locked;
        c
    }

    fn custom_with_bin(&self, c: &CustomCfg) -> CustomCfg {
        let mut c = c.clone();
        let words = crate::wsexec::split_command(&c.command);
        if let (Some(first), Some(d)) = (words.first(), self.extra_bin.as_ref()) {
            let p = d.join(first);
            if !first.contains('/') && p.exists() {
                c.command = c.command.trim_start().replacen(first.as_str(), &p.display().to_string(), 1);
            }
        }
        c
    }

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

    pub fn cfg_of(&self, id: &str) -> Option<(bool, String)> {
        match id {
            CLAUDE_CODE => Some((self.claude.allow_auto, self.claude.key_ref.clone())),
            COPILOT => Some((self.copilot.allow_auto, self.copilot.key_ref.clone())),
            other => other.strip_prefix("custom:").and_then(|c| self.custom.iter().find(|x| x.id == c)).map(|c| (c.allow_auto, c.key_ref.clone())),
        }
    }
}

// ------------------------------------------------------------------------------------------------ the agent

/// How much of IYAKKI's policy the agent can be held to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// IYAKKI's tools are offered over MCP and the agent's own permission prompts come to IYAKKI.
    Bridged,
    /// The agent runs read-only as far as its flags can ask, its answer is shown, and the person applies any edits.
    Advisor,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Bridged => "policy bridge",
            Mode::Advisor => "advisor (read-only; you apply edits)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    /// The CLI says it is signed in (the detail, for example the account).
    SignedIn(String),
    /// The CLI says it is not.
    SignedOut,
    /// A key from the vault will be used.
    Key,
    /// The CLI has no way to ask; the first run will tell.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub installed: bool,
    pub version: String,
    pub auth: Auth,
    pub mode: Mode,
    /// Plain words for the chip and the picker: `ready`, `not installed`, `sign in`.
    pub chip: String,
    /// What to do when it is not ready, exact commands included.
    pub fix: String,
}

impl Status {
    pub fn ready(&self) -> bool {
        self.installed && self.auth != Auth::SignedOut
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    /// The exact command, shown to the person and run only after they agree.
    pub argv: Vec<String>,
    /// The program that must exist first.
    pub needs: &'static str,
    pub note: String,
}

/// What an agent run is asked to do.
pub struct Invocation {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The request on standard input (when it is too long for an argument, or the agent reads it there).
    pub stdin: Option<Vec<u8>>,
}

/// Everything `invocation` needs to know about a run.
pub struct Plan<'a> {
    pub workspace: &'a Path,
    pub prompt: &'a str,
    pub system: &'a str,
    pub model: &'a str,
    pub resume: Option<&'a str>,
    /// The MCP config file and the name of the tool that answers permission prompts, when the bridge is up.
    pub mcp_config: Option<&'a Path>,
    pub key: Option<&'a str>,
    pub max_turns: usize,
    /// A run that may only write text (TALES): the agent's own tools are switched off where the CLI allows it.
    pub no_tools: bool,
}

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

/// A line, safe to show: control characters out, cut to `n` characters.
pub fn tidy(s: &str, n: usize) -> String {
    let t: String = s.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect();
    if t.chars().count() > n {
        format!("{}…", t.chars().take(n).collect::<String>())
    } else {
        t
    }
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

fn now_secs() -> u64 {
    crate::clock::now_unix().max(0) as u64
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

/// One line of the model list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    /// `agent:<id>`, the provider column of the list.
    pub provider: String,
    pub model: String,
    pub label: String,
    /// `ready`, `not installed`, `sign in`, `checking…`.
    pub chip: String,
    pub ready: bool,
    pub fix: String,
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

/// For the "test" button and anything else that needs a `Tools` with nothing in it.
pub struct NoTools;

impl crate::agent::Tools for NoTools {
    fn defs(&self) -> Vec<crate::agent::ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _args: &Value) -> Result<String, String> {
        Err(format!("no tool called {name}"))
    }
    fn describe(&self, name: &str, _args: &Value) -> String {
        name.into()
    }
}
