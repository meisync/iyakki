//! The assistant's tools for the MCP servers of the MCP tile (the keys N and D of the tile had no tool): `mcp_servers` (free: the servers this account
//! may use and where each comes from), `mcp_server_add` and `mcp_server_remove` (each asks every time, administrators only, never covered by a
//! standing grant: a server is a program that is started on this computer). The servers of the administrator's `mcp.json` cannot be removed here, as in
//! the tile. Which servers an account sees is decided in [`crate::mcp::visible_servers`], the same code the tile uses.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::mcp::{self, ServerSpec, StoredServer};
pub use iyakki_shared::mcpadmintools::{PROMPT_RULE};

pub const TOOLS: [&str; 3] = ["mcp_servers", "mcp_server_add", "mcp_server_remove"];


pub trait Backend: Send + Sync {
    fn can_manage(&self) -> bool;
    /// What this account may use, by name with its origin.
    fn servers(&self) -> Vec<(ServerSpec, &'static str)>;
    fn add(&self, s: StoredServer) -> Result<(), String>;
    fn remove(&self, name: &str) -> Result<(), String>;
    fn audit(&self, action: &str, subject: &str);
}

pub struct McpAdminTools {
    inner: Arc<dyn Tools>,
    be: Arc<dyn Backend>,
}

impl McpAdminTools {
    pub fn new(inner: Arc<dyn Tools>, be: Arc<dyn Backend>) -> McpAdminTools {
        McpAdminTools { inner, be }
    }

    fn gate(&self) -> Result<(), String> {
        if self.be.can_manage() {
            Ok(())
        } else {
            Err("only an administrator account may add or remove MCP servers".into())
        }
    }

    fn new_server(&self, a: &Value) -> Result<StoredServer, String> {
        self.gate()?;
        let name = a["name"].as_str().ok_or("give the server's name (name)")?.trim().to_string();
        if !mcp::valid_server_name(&name) {
            return Err("a server name is letters, digits, - and _ (up to 32)".into());
        }
        let command = a["command"].as_str().ok_or("give the program that starts the server (command)")?.trim().to_string();
        if command.is_empty() || command.contains('\0') || command.contains('\n') {
            return Err("give the program that starts the server (command): one path, no new line".into());
        }
        let args: Vec<String> = match a.get("args") {
            None | Some(Value::Null) => vec![],
            Some(Value::Array(v)) => v.iter().map(|x| x.as_str().map(str::to_string).ok_or("each argument is text")).collect::<Result<_, _>>()?,
            Some(_) => return Err("args is a list of text".into()),
        };
        if args.iter().any(|x| x.contains('\0') || x.len() > 500) || args.len() > 40 {
            return Err("at most 40 arguments of up to 500 characters".into());
        }
        if self.be.servers().iter().any(|(s, _)| s.name == name) {
            return Err(format!("a server called {name} exists already"));
        }
        Ok(StoredServer { name, command, args, sample: false })
    }

    fn removable(&self, a: &Value) -> Result<String, String> {
        self.gate()?;
        let name = a["name"].as_str().ok_or("give the server's name (name)")?.trim().to_string();
        match self.be.servers().iter().find(|(s, _)| s.name == name) {
            None => Err(format!("there is no server called {name}")),
            Some((_, "mcp.json")) => Err(format!("{name} comes from the administrator's mcp.json; remove it from that file")),
            Some(_) => Ok(name),
        }
    }
}

impl Tools for McpAdminTools {
    fn inner_tools(&self) -> Option<&dyn Tools> {
        Some(&*self.inner)
    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !TOOLS.contains(&d.spec.name.as_str()));
        let obj = |p: Value, r: &[&str]| json!({"type": "object", "properties": p, "required": r});
        v.push(ToolDef { spec: ToolSpec { name: TOOLS[0].into(), description: "List the MCP servers this account may use, with where each comes from (sample, added here, mcp.json).".into(), schema: obj(json!({}), &[]) }, risk: Risk::Safe, untrusted: true });
        v.push(ToolDef { spec: ToolSpec { name: TOOLS[1].into(), description: "Add an MCP server: `name`, `command` (the program that starts it, a path) and `args` (a list of text). The server is a program that is started on this computer when someone connects, so the person is always asked, with the command on the card.".into(), schema: obj(json!({"name": {"type": "string"}, "command": {"type": "string"}, "args": {"type": "array", "items": {"type": "string"}}}), &["name", "command"]) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: TOOLS[2].into(), description: "Remove an MCP server that was added here (not one of the administrator's mcp.json). The person is always asked.".into(), schema: obj(json!({"name": {"type": "string"}}), &["name"]) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "mcp_servers" => {
                let s = self.be.servers();
                if s.is_empty() {
                    return Ok("No MCP servers are available to this account.".into());
                }
                Ok(s.iter().map(|(x, o)| format!("{} ({o}): {}{}", x.name, x.command, if x.args.is_empty() { String::new() } else { format!(" {}", x.args.join(" ")) })).collect::<Vec<_>>().join("\n"))
            }
            "mcp_server_add" => {
                let s = self.new_server(args)?;
                let n = s.name.clone();
                self.be.add(s)?;
                self.be.audit("mcp.add", &n);
                Ok(format!("Added the MCP server {n}. Connect to it in the MCP app; its tools are then called as mcp__{n}__TOOL."))
            }
            "mcp_server_remove" => {
                let n = self.removable(args)?;
                self.be.remove(&n)?;
                self.be.audit("mcp.remove", &n);
                Ok(format!("Removed the MCP server {n}."))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            "mcp_server_add" => format!("Add the MCP server “{}” (it starts the program {})", args["name"].as_str().unwrap_or("?"), args["command"].as_str().unwrap_or("?")),
            "mcp_server_remove" => format!("Remove the MCP server “{}”", args["name"].as_str().unwrap_or("?")),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            "mcp_server_add" => {
                let s = self.new_server(args).ok()?;
                Some(format!("Server: {}\nProgram: {}\nArguments: {}\nIt is started on this computer, with your rights, each time someone connects to it.", s.name, s.command, if s.args.is_empty() { "none".to_string() } else { s.args.join(" ") }))
            }
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            "mcp_server_add" => self.new_server(args).err(),
            "mcp_server_remove" => self.removable(args).err(),
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !TOOLS.contains(&name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == TOOLS[1] || name == TOOLS[2] { Some("adding a program that is started on this computer always asks".into()) } else { self.inner.grant_blocker(name, args) }
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
}

/// The real backend: the account's services, policy and audit log.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub policy: iyakki_sec::policy::Policy,
    pub user: String,
    /// The servers of the administrator's `mcp.json`.
    pub file_servers: Vec<ServerSpec>,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Backend for RealBackend {
    fn can_manage(&self) -> bool {
        self.policy.manage_settings
    }
    fn servers(&self) -> Vec<(ServerSpec, &'static str)> {
        let stored = self.svc.mcp.list();
        mcp::visible_servers(&self.file_servers, &stored, &self.policy).into_iter().map(|s| { let o = mcp::origin_of(&stored, &s.name); (s, o) }).collect()
    }
    fn add(&self, s: StoredServer) -> Result<(), String> {
        self.svc.mcp.add(s)
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.svc.mcp.remove(name)
    }
    fn audit(&self, action: &str, subject: &str) {
        (self.audit)(action, subject)
    }
}

#[cfg(test)]
#[path = "mcpadmintools_tests.rs"]
mod tests;
