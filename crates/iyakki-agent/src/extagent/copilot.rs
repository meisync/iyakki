//! GitHub Copilot CLI (`copilot`), non-interactive: `copilot -p PROMPT --output-format=json`.
//! Verified live on 2026-10-19 (Copilot CLI 1.0.98, musl build, a stand-in stdio MCP server; evidence in docs/EXTERNAL-AGENTS.md):
//! * `--additional-mcp-config=@FILE` takes `{"mcpServers":{"iyakki":{"type":"stdio","command","args","env","tools":["*"],"timeout":ms}}}`.
//! * An MCP tool is shown to the model as `<server>-<tool>`; `--allow-tool=iyakki` (server) or `iyakki(tool)` allows it without a prompt.
//! * `--available-tools=view,glob,grep,iyakki` leaves the model exactly those (no bash, create, edit, web_fetch, task, sql, ...).
//! * A tool that is not allowed is refused at once in `-p` mode (`tool.execution_complete` with `error.code` "denied"); there is no
//!   approval event to answer. So approvals are not Copilot's: the one allowed server is IYAKKI's, whose gate asks the person.
//! * JSONL: `assistant.message_delta` (`data.deltaContent`), `assistant.message` (`data.content`, `data.toolRequests`),
//!   `tool.execution_start` (`toolCallId`, `toolName`, `mcpToolName`, `arguments`), `tool.execution_complete` (`success`,
//!   `result.content` or `error.message`), last line `result` (`sessionId`, `exitCode`, `usage.premiumRequests`); `session.*`,
//!   `model.*`, `assistant.turn_*` and `assistant.tool_call_delta` are bookkeeping.
//! Without the bridge (Settings → Agents, "advisor only") it runs read-only: `view`, `glob`, `grep`, nothing else.

use serde_json::Value;

use super::{valid_model_id, Auth, AgentCfg, Ev, ModelInfo, ModelSource, ExternalAgent, Install, Invocation, Mode, ParseState, Plan};

pub struct Copilot {
    cfg: AgentCfg,
}

impl Copilot {
    pub fn new(cfg: AgentCfg) -> Copilot {
        Copilot { cfg }
    }
}

/// The text in one JSONL record, whatever the exact event name: the first of the usual fields that holds a string.
fn text_of(v: &Value) -> Option<String> {
    for p in ["/data/content", "/data/deltaContent", "/data/text", "/content", "/text", "/delta", "/message", "/result"] {
        match v.pointer(p) {
            Some(Value::String(s)) if !s.is_empty() => return Some(s.clone()),
            _ => {}
        }
    }
    None
}

impl ExternalAgent for Copilot {
    fn id(&self) -> String {
        super::COPILOT.into()
    }
    fn label(&self) -> String {
        "GitHub Copilot".into()
    }
    fn program(&self) -> String {
        if self.cfg.command.trim().is_empty() { "copilot".into() } else { self.cfg.command.trim().into() }
    }
    fn cfg_key_ref(&self) -> String {
        self.cfg.key_ref.clone()
    }
    fn cfg_model(&self) -> String {
        self.cfg.model.clone()
    }
    /// Copilot CLI's `--model` accepts the ids its own `/model` picker shows; its documentation names no ids and no command that
    /// prints them without a session, so only "default" is shipped (the person can type an id they saw in `/model`).
    fn builtin_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo::new("", "Default (let the CLI choose)", "Copilot's own choice for your plan; /model inside the CLI shows the rest", ModelSource::BuiltIn)]
    }
    fn custom_models(&self) -> Vec<String> {
        self.cfg.custom_models.clone()
    }
    fn mode(&self) -> Mode {
        if self.cfg.advisor_only { Mode::Advisor } else { Mode::Bridged }
    }
    fn parse_auth(&self, _o: &crate::wsexec::Out) -> Auth {
        Auth::Unknown
    }
    /// GitHub documents no status command, so the program's own files are looked at (`config.json` in `~/.copilot`, or `$COPILOT_HOME`)
    /// and the token variables it checks; nothing is called and no token is read.
    fn auth_offline(&self) -> Auth {
        // a token kept in the person's vault: IYAKKI hands it to the program when it starts it
        if let Some(l) = &self.cfg.vault_login {
            return Auth::SignedIn(format!("{l} (token in your vault)"));
        }
        let home = self.cfg.env.iter().find(|(k, _)| k == "HOME").map(|(_, v)| v.clone()).or_else(|| std::env::var("HOME").ok());
        let r = copilot_files_auth(std::env::var("COPILOT_HOME").ok().as_deref(), home.as_deref(), &|k| std::env::var(k).ok());
        // the token may sit in a vault that is locked: then nobody can say
        if r == Auth::SignedOut && self.cfg.vault_locked {
            return Auth::Unknown;
        }
        r
    }
    fn child_env(&self) -> Vec<(String, String)> {
        self.cfg.env.clone()
    }
    fn key_env(&self) -> Option<String> {
        // a fine-grained token with the "Copilot Requests" permission; checked first by the CLI
        Some("COPILOT_GITHUB_TOKEN".into())
    }
    fn install(&self) -> Option<Install> {
        Some(Install { argv: vec!["npm".into(), "install".into(), "-g".into(), "@github/copilot".into()], needs: "node and npm", note: "See https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli.".into() })
    }
    fn login(&self) -> Option<Vec<String>> {
        Some(vec![self.program(), "login".into()])
    }
    fn login_help(&self) -> String {
        format!("Sign in first: press Login next to GitHub Copilot (the CLI list, Settings → Agents or the model list), or run `{} login` in a terminal (or start `{}` and use /login). A GitHub Copilot subscription is needed. IYAKKI never sees your password; a token from the vault can be used instead.", self.program(), self.program())
    }

    fn invocation(&self, p: &Plan) -> Invocation {
        let mut a: Vec<String> = vec![self.program(), "--output-format=json".into(), "--no-ask-user".into(), "--no-auto-update".into(), format!("--add-dir={}", p.workspace.display())];
        if !p.model.is_empty() && p.model != "default" && valid_model_id(p.model) {
            a.push(format!("--model={}", p.model));
        }
        if let Some(sid) = p.resume.filter(|s| valid_model_id(s)) {
            a.push(format!("--resume={sid}"));
        }
        // the model sees only the read tools of the CLI and, in the bridge, the one server whose gate asks the person
        let mut avail = READ_TOOLS.join(",");
        if let (Mode::Bridged, Some(cfg)) = (self.mode(), p.mcp_config) {
            a.push(format!("--additional-mcp-config=@{}", super::bridge::copilot_config(cfg).display()));
            a.push("--disable-builtin-mcps".into());
            // the whole server is allowed: IYAKKI's gate decides per tool (policy, grants, approval card)
            a.push(format!("--allow-tool={}", super::claude_code::SERVER));
            avail.push(',');
            avail.push_str(super::claude_code::SERVER);
            if p.no_tools {
                avail = super::claude_code::SERVER.into();
            }
        }
        a.push(format!("--available-tools={avail}"));
        // belt and braces: denial rules beat every allow rule, even --allow-all-tools
        a.extend(["--deny-tool=write".into(), "--deny-tool=shell".into()]);
        // Copilot has no system-prompt flag: the rules go in front of the request
        let prompt = if p.system.is_empty() { p.prompt.to_string() } else { format!("{}\n\n---\n{}", p.system, p.prompt) };
        a.extend(["-p".into(), prompt]);
        let env = match (p.key, self.key_env()) {
            (Some(k), Some(name)) => vec![(name, k.to_string())],
            _ => vec![],
        };
        let mut env = env;
        env.extend(self.cfg.env.clone());
        Invocation { argv: a, env, stdin: None }
    }

    fn parse_line(&self, line: &str, st: &mut ParseState) -> Vec<Ev> {
        let line = line.trim();
        if line.is_empty() {
            return vec![];
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            // not JSON: plain text from a CLI that ignored the format
            st.bad_lines += 1;
            st.partial.push_str(line);
            st.partial.push('\n');
            return vec![Ev::Delta(st.partial.clone())];
        };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or("").to_string();
        match kind {
            "assistant.message_delta" => {
                st.partial.push_str(&s("/data/deltaContent"));
                vec![Ev::Delta(st.partial.clone())]
            }
            "assistant.message" => {
                st.partial.clear();
                let t = s("/data/content");
                if t.trim().is_empty() { vec![] } else { vec![Ev::Text(t)] }
            }
            "tool.execution_start" => {
                let id = s("/data/toolCallId");
                // an IYAKKI tool is named by the tool, not `iyakki-<tool>`
                let name = Some(s("/data/mcpToolName")).filter(|n| !n.is_empty() && s("/data/mcpServerName") == super::claude_code::SERVER).unwrap_or_else(|| s("/data/toolName"));
                if st.announced.insert(id.clone()) {
                    vec![Ev::ToolUse { id, name: if name.is_empty() { "tool".into() } else { name }, input: v.pointer("/data/arguments").cloned().unwrap_or(Value::Null) }]
                } else {
                    vec![]
                }
            }
            "tool.execution_complete" => {
                let ok = v.pointer("/data/success").and_then(Value::as_bool).unwrap_or(true);
                let text = if ok { s("/data/result/content") } else { Some(s("/data/error/message")).filter(|m| !m.is_empty()).unwrap_or_else(|| s("/data/result/content")) };
                vec![Ev::ToolResult { id: s("/data/toolCallId"), ok, text }]
            }
            "result" => {
                st.seen_result = true;
                let code = v.get("exitCode").and_then(Value::as_i64).unwrap_or(0);
                vec![Ev::Done { ok: code == 0, text: String::new(), session: Some(s("/sessionId")).filter(|x| !x.is_empty()), cost_usd: None, input: 0, output: 0, cached: 0, error: (code != 0).then(|| format!("{} exited with code {code}", self.program())) }]
            }
            k if k.contains("error") && !k.starts_with("session.mcp") => {
                vec![Ev::Done { ok: false, text: String::new(), session: None, cost_usd: None, input: 0, output: 0, cached: 0, error: Some(text_of(&v).or_else(|| Some(s("/data/message")).filter(|m| !m.is_empty())).unwrap_or_else(|| line.to_string())) }]
            }
            _ => vec![],
        }
    }
}

/// The CLI's own tools that only read (names from the live `--available-tools` check).
pub const READ_TOOLS: [&str; 3] = ["view", "glob", "grep"];

/// What the files of the Copilot CLI say about who is signed in. UNVERIFIED against a live CLI: the shape of `config.json`
/// (`logged_in_users` / `last_logged_in_user` with a `login`) is what has been seen, and it is read leniently; when the shape is
/// not recognised the answer is `Unknown`, never `SignedOut`.
pub fn copilot_files_auth(copilot_home: Option<&str>, home: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Auth {
    for k in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        if env(k).is_some_and(|v| !v.is_empty()) {
            return Auth::SignedIn(format!("token from ${k}"));
        }
    }
    let dir = match (copilot_home.filter(|h| !h.is_empty()), home.filter(|h| !h.is_empty())) {
        (Some(c), _) => std::path::PathBuf::from(c),
        (None, Some(h)) => std::path::Path::new(h).join(".copilot"),
        _ => return Auth::Unknown,
    };
    let Ok(text) = std::fs::read_to_string(dir.join("config.json")) else { return if dir.is_dir() { Auth::Unknown } else { Auth::SignedOut } };
    // the file starts with `//` comment lines
    let json: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    let Ok(v) = serde_json::from_str::<Value>(&json) else { return Auth::Unknown };
    let login_of = |u: &Value| u.get("login").and_then(Value::as_str).filter(|l| !l.is_empty() && l.len() < 80 && !l.chars().any(char::is_control)).map(str::to_string);
    if let Some(l) = v.get("last_logged_in_user").or_else(|| v.get("lastLoggedInUser")).and_then(login_of) {
        return Auth::SignedIn(l);
    }
    match v.get("logged_in_users").or_else(|| v.get("loggedInUsers")).and_then(Value::as_array) {
        Some(a) => match a.iter().find_map(login_of) {
            Some(l) => Auth::SignedIn(l),
            None => Auth::SignedOut,
        },
        None => Auth::Unknown,
    }
}
