//! The lead assistant's tools for the interactive SSH login of the CLI tile (F-CLI-08, `docs/CLI-SSH.md`): `ssh_connections` lists the
//! person's saved hosts (free, no secrets) and `ssh_login` asks to open one of them as a real terminal session in the CLI tile.
//!
//! * `ssh_login` is on the hard-ask list (`agent::hard_ask`): it asks every time, in Auto mode too, and an "allow for this session"
//!   grant never covers it. The card names the connection, `user@host:port` and says that the person types in the session.
//! * The assistant **cannot type in, read or see the session**. The tool only asks the CLI tile to open the login; the tile then
//!   belongs to the person. (Commands the assistant wants run go through `run_command`, which asks and is output-redacted.)
//! * The same rules as the tile's `L` key apply, checked here before anything is asked and again by the tile: a full terminal, a host
//!   the account may reach, a host key that is already trusted (the assistant can never trust one) and an SSH client on the machine.
//!   Keys only: the login runs with `BatchMode=yes`, so no password is ever asked or typed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::ssh::{Connection, Env};

pub const LIST_TOOL: &str = "ssh_connections";
pub const LOGIN_TOOL: &str = "ssh_login";

pub const PROMPT_RULE: &str = "\nSSH LOGIN: ssh_connections lists the person's saved SSH hosts (name, user@host:port, key name, whether the host key is trusted). ssh_login opens one of them as an interactive terminal in the CLI tile after the person approves a card; the person types in that session, you cannot type in it, read it or see it. For a command whose output you need, use run_command with ssh instead. You cannot trust a new host key: if ssh_login says it is not trusted, tell the person to press T on the Connections tab of the CLI tile. Passwords are never used (keys only).";

pub fn is_session_tool(n: &str) -> bool {
    n == LIST_TOOL || n == LOGIN_TOOL
}

type Requests = Mutex<HashMap<(PathBuf, String), Vec<String>>>;

fn requests() -> &'static Requests {
    static R: OnceLock<Requests> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The assistant (after the person approved) asks the CLI tile of `user` to open the login of the saved connection `name`.
pub fn request_login(data_dir: &Path, user: &str, name: &str) {
    let mut m = requests().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let q = m.entry((data_dir.to_path_buf(), user.to_string())).or_default();
    if !q.iter().any(|n| n == name) {
        q.push(name.to_string());
    }
}

/// Whether a login is waiting to be opened (the shell then brings the CLI tile up).
pub fn pending(data_dir: &Path, user: &str) -> bool {
    let m = requests().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    m.get(&(data_dir.to_path_buf(), user.to_string())).is_some_and(|q| !q.is_empty())
}

/// The CLI tile takes the oldest waiting request.
pub fn take_login(data_dir: &Path, user: &str) -> Option<String> {
    let mut m = requests().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let q = m.get_mut(&(data_dir.to_path_buf(), user.to_string()))?;
    if q.is_empty() { None } else { Some(q.remove(0)) }
}

pub type EnvFn = Arc<dyn Fn() -> Env + Send + Sync>;
pub type AuditFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub struct SshSessionTools {
    inner: Arc<dyn Tools>,
    env: EnvFn,
    data_dir: PathBuf,
    user: String,
    /// The account has a full terminal (`CliLevel::Full`).
    full_terminal: bool,
    allows_host: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    audit: AuditFn,
}

impl SshSessionTools {
    pub fn new(inner: Arc<dyn Tools>, env: EnvFn, data_dir: &Path, user: &str, full_terminal: bool, allows_host: Arc<dyn Fn(&str) -> bool + Send + Sync>, audit: AuditFn) -> SshSessionTools {
        SshSessionTools { inner, env, data_dir: data_dir.to_path_buf(), user: user.to_string(), full_terminal, allows_host, audit }
    }

    /// The saved connection a call names, when a login to it may be asked for now.
    fn check(&self, args: &Value) -> Result<Connection, String> {
        let name = args.get("connection").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if name.is_empty() {
            return Err("name the connection (ssh_connections lists them)".into());
        }
        if !self.full_terminal {
            return Err("an interactive login needs a full terminal for this account; ask an administrator".into());
        }
        let env = (self.env)();
        let c = env.connections().get(&name).ok_or_else(|| format!("there is no saved connection called {name} (ssh_connections lists them; the person adds one in the CLI tile's Connections tab)"))?;
        if !(self.allows_host)(&c.host) {
            return Err(format!("this account may not reach {}", c.host));
        }
        if env.client().is_none() {
            return Err("no SSH client was found on this machine".into());
        }
        if env.known_hosts().for_host(&c.host, c.port).is_empty() {
            return Err(format!("the host key of {}:{} is not trusted yet. You cannot trust it: tell the person to open the CLI tile, Connections tab, select {} and press T to see the fingerprint and trust it.", c.host, c.port, c.name));
        }
        Ok(c)
    }

    fn list(&self) -> String {
        let env = (self.env)();
        let rows = env.connections().list();
        if rows.is_empty() {
            return "No saved SSH connections yet. The person adds one in the CLI tile, Connections tab (n).".into();
        }
        rows.iter()
            .map(|c| {
                let trusted = !env.known_hosts().for_host(&c.host, c.port).is_empty();
                let reach = (self.allows_host)(&c.host);
                format!(
                    "{} · {}:{} · key {} · host key {}{}{}",
                    c.name,
                    c.target(),
                    c.port,
                    if c.key.is_empty() { "none (the client's default)".to_string() } else { c.key.clone() },
                    if trusted { "trusted" } else { "NOT trusted yet" },
                    if reach { "" } else { " · this account may not reach it" },
                    if c.command.is_empty() { "" } else { " · has a default command" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn card(&self, args: &Value) -> Result<String, String> {
        let c = self.check(args)?;
        Ok(format!(
            "Open an interactive SSH login to “{}” ({}:{}) in the CLI tile.\nYou type in the session yourself; the assistant cannot type in it, read it or see it. Keys only: no password is asked.{}",
            c.name,
            c.target(),
            c.port,
            if c.command.is_empty() { String::new() } else { format!("\nIt runs the saved default command: {}", c.command) }
        ))
    }
}

impl Tools for SshSessionTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_session_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: LIST_TOOL.into(), description: "List the person's saved SSH connections (name, user@host:port, key name, whether the host key is trusted). No secrets.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: LOGIN_TOOL.into(),
                description: "Open an interactive SSH login to a saved connection as a terminal session in the CLI tile. The person approves a card every time; the person types in the session, you cannot type in it, read it or see it. Keys only (no passwords). The host key must already be trusted; you cannot trust one.".into(),
                schema: json!({"type": "object", "properties": {"connection": {"type": "string", "description": "The saved connection's name (ssh_connections lists them)."}}, "required": ["connection"]}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            LIST_TOOL => Ok(self.list()),
            LOGIN_TOOL => {
                let c = self.check(args)?;
                request_login(&self.data_dir, &self.user, &c.name);
                (self.audit)("ssh.login.request", &c.name);
                Ok(format!("Asked the CLI tile to open the login to “{}”. It opens on the person's screen; they type in it. You cannot see or type in the session.", c.name))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            LIST_TOOL => "List the saved SSH connections".into(),
            LOGIN_TOOL => match self.card(args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{LOGIN_TOOL} (invalid: {e})"),
            },
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            LOGIN_TOOL => self.card(args).ok(),
            LIST_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            LOGIN_TOOL => self.card(args).err(),
            LIST_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_session_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !is_session_tool(name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == LOGIN_TOOL {
            return Some("a login to another computer always asks".into());
        }
        self.inner.grant_blocker(name, args)
    }

    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }

    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }
    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}
