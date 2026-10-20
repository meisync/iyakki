//! Claude Code (`claude`), driven headless: `claude -p --output-format stream-json --verbose --include-partial-messages`.
//! Flags and event shapes: `docs/EXTERNAL-AGENTS.md` (read from code.claude.com/docs on 2026-10-14; not run against a live CLI).

use serde_json::Value;

use super::{valid_model_id, Auth, AgentCfg, Ev, ModelInfo, ModelSource, ExternalAgent, Install, Invocation, Mode, ParseState, Plan};
use crate::wsexec::Out;

pub struct ClaudeCode {
    cfg: AgentCfg,
}

/// The name of the MCP server IYAKKI offers and of the tool that answers Claude Code's permission prompts.
pub const SERVER: &str = "iyakki";
pub const APPROVE_TOOL: &str = "mcp__iyakki__approve";
pub const TOOL_PREFIX: &str = "mcp__iyakki__";

/// Raw shell commands that are refused before they run: the cluster and container commands go through IYAKKI's own tools
/// (`k8s_*`, `deploy_*`, `run_command`), which know the dev cluster, the namespace and the approval rules.
pub const DENIED_BASH: [&str; 18] = ["kubectl", "k3s", "kubeadm", "minikube", "kind", "helm", "docker rm", "docker rmi", "docker system", "docker kill", "docker stop", "podman rm", "podman rmi", "podman system", "podman kill", "podman stop", "sudo", "su"];

/// Claude Code's own tools, all refused for a story (names from the CLI's documentation).
pub const ALL_OWN_TOOLS: [&str; 10] = ["Bash", "Read", "Edit", "Write", "MultiEdit", "NotebookEdit", "Glob", "Grep", "WebFetch", "WebSearch"];

impl ClaudeCode {
    pub fn new(cfg: AgentCfg) -> ClaudeCode {
        ClaudeCode { cfg }
    }

    /// `--disallowedTools` rules for the raw shell. Pattern form `Bash(prog *)` (a space before `*` matches whole words).
    pub fn disallowed() -> String {
        DENIED_BASH.iter().map(|p| format!("Bash({p} *)")).chain(std::iter::once("Bash(kubectl)".to_string())).collect::<Vec<_>>().join(",")
    }
}

impl ExternalAgent for ClaudeCode {
    fn id(&self) -> String {
        super::CLAUDE_CODE.into()
    }
    fn label(&self) -> String {
        "Claude Code".into()
    }
    fn program(&self) -> String {
        if self.cfg.command.trim().is_empty() { "claude".into() } else { self.cfg.command.trim().into() }
    }
    fn cfg_key_ref(&self) -> String {
        self.cfg.key_ref.clone()
    }
    fn cfg_model(&self) -> String {
        self.cfg.model.clone()
    }
    /// The aliases of `--model` from <https://code.claude.com/docs/en/model-config> (checked 2026-10-16). Claude Code has no
    /// documented command that lists models without a session, so this list is what is shown unless the person types an id.
    fn builtin_models(&self) -> Vec<ModelInfo> {
        let b = |id: &str, label: &str, note: &str| ModelInfo::new(id, label, note, ModelSource::BuiltIn);
        vec![
            b("", "Default (let the CLI choose)", "the model of your account and plan"),
            b("sonnet", "Sonnet", "daily coding tasks; fast"),
            b("opus", "Opus", "complex reasoning; most capable of the standard three"),
            b("haiku", "Haiku", "fast and cheap, simple tasks"),
            b("fable", "Fable", "hardest, longest tasks; may need usage credits"),
            b("opusplan", "Opus plan + Sonnet", "Opus plans, Sonnet carries it out"),
        ]
    }
    fn custom_models(&self) -> Vec<String> {
        self.cfg.custom_models.clone()
    }
    fn mode(&self) -> Mode {
        Mode::Bridged
    }
    fn auth_args(&self) -> Option<Vec<String>> {
        Some(vec!["auth".into(), "status".into()])
    }
    /// `claude auth status` prints JSON and exits 0 when signed in, 1 when not.
    fn parse_auth(&self, o: &Out) -> Auth {
        if o.missing {
            return Auth::Unknown;
        }
        let v: Option<Value> = serde_json::from_str(o.stdout.trim()).ok();
        let method = v.as_ref().and_then(|v| v.get("authMethod")).and_then(Value::as_str).unwrap_or("");
        let logged = v.as_ref().and_then(|v| v.get("loggedIn")).and_then(Value::as_bool);
        match (o.code, logged, method) {
            (_, Some(false), _) | (_, _, "none") => Auth::SignedOut,
            (Some(0), _, m) => {
                // the account, when `auth status` names one (an address is shown to the person who is signed in, nowhere else)
                let who = v.as_ref().and_then(|v| v.get("email")).and_then(Value::as_str).filter(|e| !e.is_empty() && e.len() < 120 && !e.chars().any(char::is_control));
                Auth::SignedIn(match (who, m.is_empty()) {
                    (Some(e), false) => format!("{e}, {m}"),
                    (Some(e), true) => e.to_string(),
                    (None, false) => m.to_string(),
                    (None, true) => "signed in".to_string(),
                })
            }
            (Some(1), _, _) => Auth::SignedOut,
            _ => Auth::Unknown,
        }
    }
    fn child_env(&self) -> Vec<(String, String)> {
        self.cfg.env.clone()
    }
    fn key_env(&self) -> Option<String> {
        Some("ANTHROPIC_API_KEY".into())
    }
    fn install(&self) -> Option<Install> {
        Some(Install {
            argv: vec!["npm".into(), "install".into(), "-g".into(), "@anthropic-ai/claude-code".into()],
            needs: "node and npm",
            note: "Anthropic also offers a native installer; see https://code.claude.com/docs/en/setup.".into(),
        })
    }
    fn login(&self) -> Option<Vec<String>> {
        Some(vec![self.program(), "auth".into(), "login".into()])
    }
    fn login_help(&self) -> String {
        format!("Sign in first: press Login next to Claude Code (the CLI list, Settings → Agents or the model list), or run `{} auth login` in a terminal (add --console to bill an API account), or add an API key from the vault in Settings → Agents. IYAKKI never sees your password.", self.program())
    }

    fn invocation(&self, p: &Plan) -> Invocation {
        let mut a: Vec<String> = vec![self.program(), "-p".into(), "--output-format".into(), "stream-json".into(), "--verbose".into(), "--include-partial-messages".into()];
        // the caller decides the model (the chat's choice, else the agent's setting); empty means the CLI chooses
        if !p.model.is_empty() && p.model != "default" && valid_model_id(p.model) {
            a.extend(["--model".into(), p.model.into()]);
        }
        if let Some(sid) = p.resume {
            a.extend(["--resume".into(), sid.into()]);
        }
        a.extend(["--add-dir".into(), p.workspace.display().to_string()]);
        // a project's own .claude/settings.json may still apply; the person's global allow rules do not
        a.extend(["--setting-sources".into(), "project".into()]);
        a.extend(["--permission-mode".into(), "default".into()]);
        a.extend(["--max-turns".into(), p.max_turns.max(1).to_string()]);
        a.push(format!("--disallowedTools={}", if p.no_tools { format!("{},{}", Self::disallowed(), ALL_OWN_TOOLS.join(",")) } else { Self::disallowed() }));
        if let Some(cfg) = p.mcp_config {
            a.extend(["--mcp-config".into(), cfg.display().to_string(), "--strict-mcp-config".into()]);
            a.extend(["--permission-prompt-tool".into(), APPROVE_TOOL.into()]);
            a.push(format!("--allowedTools={TOOL_PREFIX}*"));
        }
        if !p.system.is_empty() {
            a.extend(["--append-system-prompt".into(), p.system.into()]);
        }
        let long = p.prompt.len() > 60_000;
        if !long {
            a.push("--".into());
            a.push(p.prompt.into());
        }
        let env = match (p.key, self.key_env()) {
            (Some(k), Some(name)) => vec![(name, k.to_string())],
            _ => vec![],
        };
        let mut env = env;
        env.extend(self.cfg.env.clone());
        Invocation { argv: a, env, stdin: long.then(|| p.prompt.as_bytes().to_vec()) }
    }

    fn parse_line(&self, line: &str, st: &mut ParseState) -> Vec<Ev> {
        let line = line.trim();
        if line.is_empty() {
            return vec![];
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            st.bad_lines += 1;
            return vec![Ev::Note(format!("(not JSON) {}", super::tidy(line, 200)))];
        };
        let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or("").to_string();
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "system" => match s("/subtype").as_str() {
                "init" => vec![Ev::Init { session: s("/session_id"), model: s("/model") }],
                "api_retry" => vec![Ev::Note(format!("retrying after {} (attempt {} of {})", s("/error"), v.get("attempt").and_then(Value::as_u64).unwrap_or(0), v.get("max_retries").and_then(Value::as_u64).unwrap_or(0)))],
                _ => vec![],
            },
            "stream_event" => {
                // sub-agent streams carry a parent id; only the main conversation is shown as it is written
                let d = v.pointer("/event/delta");
                match d.and_then(|d| d.get("type")).and_then(Value::as_str) {
                    Some("text_delta") => {
                        st.partial.push_str(d.and_then(|d| d.get("text")).and_then(Value::as_str).unwrap_or(""));
                        vec![Ev::Delta(st.partial.clone())]
                    }
                    Some("thinking_delta") => {
                        st.thinking.push_str(d.and_then(|d| d.get("thinking")).and_then(Value::as_str).unwrap_or(""));
                        vec![Ev::Thinking(st.thinking.clone())]
                    }
                    _ => vec![],
                }
            }
            "assistant" => {
                st.partial.clear();
                st.thinking.clear();
                let mut out = vec![];
                for b in v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                    match b.get("type").and_then(Value::as_str).unwrap_or("") {
                        "thinking" => out.push(Ev::Thinking(b.get("thinking").and_then(Value::as_str).unwrap_or("").into())),
                        "text" => {
                            let t = b.get("text").and_then(Value::as_str).unwrap_or("");
                            if !t.trim().is_empty() {
                                out.push(Ev::Text(t.into()));
                            }
                        }
                        "tool_use" => {
                            let id = b.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                            if st.announced.insert(id.clone()) {
                                out.push(Ev::ToolUse { id, name: b.get("name").and_then(Value::as_str).unwrap_or("").into(), input: b.get("input").cloned().unwrap_or(Value::Null) });
                            }
                        }
                        _ => {}
                    }
                }
                out
            }
            "user" => v
                .pointer("/message/content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
                .map(|b| {
                    let text = match b.get("content") {
                        Some(Value::String(t)) => t.clone(),
                        Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
                        _ => String::new(),
                    };
                    Ev::ToolResult { id: b.get("tool_use_id").and_then(Value::as_str).unwrap_or("").into(), ok: !b.get("is_error").and_then(Value::as_bool).unwrap_or(false), text }
                })
                .collect(),
            "result" => {
                st.seen_result = true;
                let u = |k: &str| v.pointer(&format!("/usage/{k}")).and_then(Value::as_u64).unwrap_or(0);
                let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false) || s("/subtype").starts_with("error");
                let text = s("/result");
                let error = is_error.then(|| if text.is_empty() { s("/subtype") } else { text.clone() });
                vec![Ev::Done {
                    ok: !is_error,
                    text,
                    session: Some(s("/session_id")).filter(|x| !x.is_empty()),
                    cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
                    input: u("input_tokens") + u("cache_creation_input_tokens") + u("cache_read_input_tokens"),
                    output: u("output_tokens"),
                    cached: u("cache_read_input_tokens"),
                    error,
                }]
            }
            _ => vec![],
        }
    }
}
