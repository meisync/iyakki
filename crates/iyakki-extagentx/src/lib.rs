//! The plain data of the external agents (Claude Code, GitHub Copilot, custom commands), below the agent: ids, settings, the model list,
//! the status of an agent, one line of the model list. No process, no file, no network. The agent re-exports it all from
//! `iyakki_agent::extagent`; the UI names it here. See `docs/development/UI-BOUNDARY.md`, section 11, and F-SPLIT-03.

use std::path::Path;

use serde::{Deserialize, Serialize};

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
    /// The agent writes PDF files with its own file tools, so IYAKKI does not offer `pdf_create` to it (docs/PDF.md).
    pub native_pdf_output: bool,
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
        AgentCfg { enabled: true, command: String::new(), model: String::new(), custom_models: vec![], key_ref: String::new(), allow_auto: false, advisor_only: false, native_pdf_output: false, env: vec![], vault_login: None, vault_locked: false }
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
    /// The program writes PDF files itself, so `pdf_create` is not offered to it.
    pub native_pdf_output: bool,
}

impl Default for CustomCfg {
    fn default() -> Self {
        CustomCfg { id: String::new(), name: String::new(), command: String::new(), model: String::new(), models_command: String::new(), custom_models: vec![], key_ref: String::new(), key_env: String::new(), allow_auto: false, native_pdf_output: false }
    }
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

/// How much of IYAKKI's policy the agent can be held to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// A line, safe to show: control characters out, cut to `n` characters.
pub fn tidy(s: &str, n: usize) -> String {
    let t: String = s.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect();
    if t.chars().count() > n {
        format!("{}…", t.chars().take(n).collect::<String>())
    } else {
        t
    }
}

/// One line of the model list.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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


/// The sign-in data and the pure checks of the external agents' login (moved out of `iyakki_agent::extagent::login`).
pub mod login {
    use std::path::{Path, PathBuf};


    /// How the person signs in to Claude Code.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Method {
        /// A Claude subscription (Pro, Max, Team, Enterprise): `--claudeai`.
        Subscription,
        /// The Anthropic Console, billed per use through the API: `--console`.
        Console,
        /// Single sign-on of an organization: `--sso`.
        Sso,
    }

    impl Method {
        pub const ALL: [Method; 3] = [Method::Subscription, Method::Console, Method::Sso];
        pub fn label(self) -> &'static str {
            match self {
                Method::Subscription => "Claude subscription",
                Method::Console => "Anthropic Console (API billing)",
                Method::Sso => "Single sign-on (SSO)",
            }
        }
        pub fn flag(self) -> &'static str {
            match self {
                Method::Subscription => "--claudeai",
                Method::Console => "--console",
                Method::Sso => "--sso",
            }
        }
    }

    /// What to start: the program, its arguments, the folder it runs in and the environment it gets (nothing else is inherited but the
    /// search path, language and proxy settings).
    #[derive(Clone, Debug)]
    pub struct Spec {
        pub label: String,
        pub argv: Vec<String>,
        pub env: Vec<(String, String)>,
        pub cwd: PathBuf,
        /// Does the vendor flow ask the person to type something back (a code shown by the browser)?
        pub takes_code: bool,
    }

    /// What a pasted code looks like after cleaning, and what is wrong with it, before anything is sent.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct CodeCheck {
        /// What would be sent to the program.
        pub code: String,
        pub len: usize,
        /// Why it must not be sent (spaces, characters no code has, nothing there).
        pub problem: Option<String>,
        /// A doubt that does not block sending (it looks short; Claude's code usually has a `#`).
        pub warning: Option<String>,
        /// The person pasted the whole redirect address and the code was taken out of it.
        pub from_url: bool,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Phase {
        /// Started; no address yet.
        Starting,
        /// The address (and perhaps a code) is shown; the person is signing in in a browser.
        Waiting,
        /// The person's answer was sent; waiting for the program.
        Finishing,
        Succeeded,
        /// With the reason in plain words.
        Failed(String),
        Cancelled,
    }

    impl Phase {
        pub fn finished(&self) -> bool {
            matches!(self, Phase::Succeeded | Phase::Failed(_) | Phase::Cancelled)
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Snapshot {
        pub phase: Phase,
        pub url: Option<String>,
        pub code: Option<String>,
        /// The program waits for something typed (a code from the browser or a yes/no).
        pub wants_input: bool,
        pub question: Option<String>,
        /// The program answered what was sent without finishing (it refused the code and asks again): its words, plain.
        pub note: Option<String>,
        /// One live line for the person.
        pub line: String,
    }

    /// The command for an agent id (`claude-code`, `copilot`). `program` is the installed program.
    pub fn spec(agent: &str, program: &str, method: Method, home: &Path) -> Option<Spec> {
        let mut env = vec![("HOME".to_string(), home.display().to_string()), ("TERM".to_string(), "xterm-256color".to_string())];
        match agent {
            crate::CLAUDE_CODE => {
                env.push(("CLAUDE_CONFIG_DIR".into(), home.join(".claude").display().to_string()));
                Some(Spec { label: "Claude Code".into(), argv: vec![program.into(), "auth".into(), "login".into(), method.flag().into()], env, cwd: home.to_path_buf(), takes_code: true })
            }
            crate::COPILOT => {
                // the device-code flow works on a screen without a browser: a code to enter at github.com/login/device
                Some(Spec { label: "GitHub Copilot".into(), argv: vec![program.into(), "login".into(), "--device-code".into()], env, cwd: home.to_path_buf(), takes_code: false })
            }
            _ => None,
        }
    }

    fn url_decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
                let h = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
                out.push(h(b[i + 1]) << 4 | h(b[i + 2]));
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Clean what was pasted: surrounding spaces, line breaks and quotes go; a whole redirect address (`…?code=X&state=Y` or `…?code=X#Y`)
    /// becomes `X#Y`; line breaks inside (a phone wrapping the code) are joined. Then check it. `want_state`: the program (Claude's) expects
    /// `code#state`.
    pub fn check_code(raw: &str, want_state: bool) -> CodeCheck {
        let mut s = raw.trim().trim_matches(|c| matches!(c, '"' | '\'' | '`' | '<' | '>' | '“' | '”')).trim().to_string();
        let mut from_url = false;
        if s.contains("://") {
            let (before_frag, frag) = match s.split_once('#') {
                Some((a, f)) => (a.to_string(), Some(f.to_string())),
                None => (s.clone(), None),
            };
            let query = before_frag.split_once('?').map(|x| x.1).unwrap_or("");
            let get = |k: &str| query.split('&').find_map(|kv| kv.split_once('=').filter(|(n, _)| *n == k).map(|(_, v)| url_decode(v)));
            match get("code") {
                Some(code) => {
                    from_url = true;
                    let state = get("state").or(frag).filter(|x| !x.is_empty());
                    s = match state {
                        Some(st) => format!("{code}#{st}"),
                        None => code,
                    };
                }
                None => return CodeCheck { code: String::new(), len: 0, problem: Some("That address has no code in it. Copy the code the page shows, not the address of the page.".into()), warning: None, from_url: false },
            }
        }
        // a phone or a terminal may have wrapped it
        s = s.chars().filter(|c| !matches!(c, '\n' | '\r' | '\t')).collect();
        let len = s.chars().count();
        let mut problem = None;
        if s.is_empty() {
            problem = Some("Nothing to send yet. Paste the code.".to_string());
        } else if s.contains(' ') {
            problem = Some("The code has spaces in it: check that you copied all of it and only it.".to_string());
        } else if let Some(c) = s.chars().find(|c| !(c.is_ascii_alphanumeric() || "_-#.~%=+/".contains(*c))) {
            problem = Some(format!("The code has a character no code has ({c}): check that you copied the right thing."));
        }
        let mut warning = None;
        if problem.is_none() {
            if len < 20 {
                warning = Some(format!("{len} characters looks too short: check that you copied all of it."));
            } else if want_state && !s.contains('#') {
                warning = Some("Claude's code usually has a # in the middle: check that you copied all of it.".to_string());
            }
        }
        CodeCheck { code: s, len, problem, warning, from_url }
    }

    /// Does this look like something a vendor page shows as a code (for the one-tap paste from the clipboard)? Strict on purpose: a long
    /// run of code characters, no spaces, not an address.
    pub fn looks_like_code(text: &str, want_state: bool) -> bool {
        let c = check_code(text, want_state);
        c.problem.is_none() && c.len >= 40 && c.len <= 2000 && (!want_state || c.code.contains('#'))
    }

    // ------------------------------------------------------------------------------------------------ the token in the person's vault

    /// The name of Copilot's entry in the person's vault. Its value is a small JSON object: the account and the token.
    pub const COPILOT_VAULT: &str = "github-copilot";
    /// The variable GitHub's Copilot CLI reads first (before `GH_TOKEN` and `GITHUB_TOKEN`).
    pub const COPILOT_TOKEN_ENV: &str = "COPILOT_GITHUB_TOKEN";

    pub fn vault_entry(login: &str, token: &str) -> String {
        serde_json::json!({ "login": login, "token": token }).to_string()
    }

    /// (account, token) from a vault entry.
    pub fn parse_vault_entry(v: &str) -> Option<(String, zeroize::Zeroizing<String>)> {
        let j: serde_json::Value = serde_json::from_str(v).ok()?;
        let token = j.get("token")?.as_str().filter(|t| !t.is_empty())?;
        Some((j.get("login").and_then(|l| l.as_str()).unwrap_or("").to_string(), zeroize::Zeroizing::new(token.to_string())))
    }

    /// What Copilot wrote to its own settings file after a login on a machine with no keychain.
    pub struct FileToken {
        pub login: String,
        pub token: zeroize::Zeroizing<String>,
    }

    fn copilot_config(home: &Path) -> PathBuf {
        home.join(".copilot").join("config.json")
    }

    /// The file's `//` comment header and its JSON.
    fn read_config(path: &Path) -> Option<(String, serde_json::Value)> {
        let text = std::fs::read_to_string(path).ok()?;
        let header: String = text.lines().filter(|l| l.trim_start().starts_with("//")).map(|l| format!("{l}\n")).collect();
        let body: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        Some((header, serde_json::from_str(&body).ok()?))
    }

    /// The token (and whose it is) in `<home>/.copilot/config.json`, read leniently: `authTokens` may be an object (values, or objects with
    /// a `token`) or an array; the account comes from `lastLoggedInUser` / `loggedInUsers` (or the snake_case spellings), else from the
    /// end of the token's key (`https://github.com:octocat`). `None` when there is no token.
    pub fn copilot_file_token(home: &Path) -> Option<FileToken> {
        let (_, v) = read_config(&copilot_config(home))?;
        let tok = |x: &serde_json::Value| x.as_str().map(str::to_string).or_else(|| x.get("token").and_then(|t| t.as_str()).map(str::to_string)).filter(|t| !t.is_empty());
        let (mut token, mut key_login) = (None, String::new());
        match v.get("authTokens").or_else(|| v.get("auth_tokens")) {
            Some(serde_json::Value::Object(m)) => {
                for (k, x) in m {
                    if let Some(t) = tok(x) {
                        token = Some(t);
                        key_login = k.rsplit(':').next().unwrap_or("").to_string();
                        break;
                    }
                }
            }
            Some(serde_json::Value::Array(a)) => token = a.iter().find_map(tok),
            _ => {}
        }
        let login_of = |u: &serde_json::Value| u.get("login").and_then(|l| l.as_str()).filter(|l| !l.is_empty()).map(str::to_string);
        let login = v.get("lastLoggedInUser").or_else(|| v.get("last_logged_in_user")).and_then(login_of).or_else(|| v.get("loggedInUsers").or_else(|| v.get("logged_in_users")).and_then(|a| a.as_array()).and_then(|a| a.iter().find_map(login_of))).unwrap_or(key_login);
        Some(FileToken { login, token: zeroize::Zeroizing::new(token?) })
    }

    /// Take the token out of the plain file and keep everything else in it (the comment header, the other settings, the account list);
    /// the file stays readable by its owner only. `users_too` (a sign-out) also forgets who was signed in.
    pub fn scrub_copilot_file(home: &Path, users_too: bool) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        use std::io::Write;
        let path = copilot_config(home);
        let Some((header, mut v)) = read_config(&path) else { return Ok(()) };
        let mut changed = false;
        for k in ["authTokens", "auth_tokens"] {
            if let Some(x) = v.get_mut(k) {
                let empty = if x.is_array() { serde_json::json!([]) } else { serde_json::json!({}) };
                if *x != empty {
                    *x = empty;
                    changed = true;
                }
            }
        }
        if users_too {
            for k in ["lastLoggedInUser", "last_logged_in_user"] {
                if v.as_object_mut().and_then(|m| m.remove(k)).is_some() {
                    changed = true;
                }
            }
            for k in ["loggedInUsers", "logged_in_users"] {
                if let Some(x) = v.get_mut(k) {
                    if *x != serde_json::json!([]) {
                        *x = serde_json::json!([]);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return Ok(());
        }
        let out = format!("{header}{}\n", serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?);
        let tmp = path.with_extension("json.tmp");
        {
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
            f.write_all(out.as_bytes()).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
    }


}
