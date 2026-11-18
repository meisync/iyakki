//! What the MCP tile asks of the rest of the system (the second tile moved onto the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`mcp.rs`) holds no `Services`, no `AppCx` and starts no process: it asks an `McpPort`. The real port ([`CxPort`]) is the only place that
//! knows the services, the account's policy and the audit log; **it also starts the server program** (`McpClient::connect` runs the server as a child
//! process, which the tile used to do itself from a thread of its own). Tests drive the tile through a fake port and a fake session.

use std::sync::mpsc::Receiver;

use iyakki_shared::mcp::{ServerSpec, StoredServer, Tool};
use iyakki_shared::mcp::{McpClient};

use super::AppCx;

/// One connection to a server: what the tile calls on it.
pub trait McpSession: Send {
    fn call(&mut self, tool: &str, args: serde_json::Value) -> Result<String, String>;
}

/// A connected server and the tools it lists.
pub type Connected = (Box<dyn McpSession>, Vec<Tool>);

pub trait McpPort {
    /// The servers this account may use: the administrator's file and the ones added in the tile, after the account's policy.
    fn visible(&self) -> Vec<ServerSpec>;
    /// Where a server comes from: `sample`, `added here` or `mcp.json`.
    fn origin(&self, name: &str) -> &'static str;
    /// May this account add and remove servers?
    fn can_manage(&self) -> bool;
    fn add(&self, server: StoredServer) -> Result<(), String>;
    fn remove(&self, name: &str) -> Result<(), String>;
    /// Start the server and ask for its tools, on a thread of its own; the answer comes on the channel.
    fn connect(&self, spec: ServerSpec) -> Receiver<Result<Connected, String>>;
    /// The signed-in person's name and role.
    fn account(&self) -> (String, String);
    /// Has the administrator configured any server at all (`mcp.json`)?
    fn configured(&self) -> bool;
    fn audit(&self, action: &str, subject: &str);
}

struct RealSession(McpClient);

impl McpSession for RealSession {
    fn call(&mut self, tool: &str, args: serde_json::Value) -> Result<String, String> {
        self.0.call(tool, args)
    }
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl McpPort for CxPort<'_> {
    fn visible(&self) -> Vec<ServerSpec> {
        crate::agent().mcp_visible_servers(&self.0.mcp_servers, &self.0.mcp().list(), &self.0.session.policy)
    }
    fn origin(&self, name: &str) -> &'static str {
        self.0.agent.b0().mcp().origin_of(&self.0.mcp().list(), name)
    }
    fn can_manage(&self) -> bool {
        self.0.session.policy.manage_settings
    }
    fn add(&self, server: StoredServer) -> Result<(), String> {
        self.0.mcp().add(server)
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.0.mcp().remove(name)
    }
    fn connect(&self, spec: ServerSpec) -> Receiver<Result<Connected, String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = McpClient::connect(&spec, std::time::Duration::from_secs(10)).and_then(|mut c| {
                let t = c.tools()?;
                Ok((Box::new(RealSession(c)) as Box<dyn McpSession>, t))
            });
            let _ = tx.send(r);
        });
        rx
    }
    fn account(&self) -> (String, String) {
        (self.0.session.user.clone(), self.0.session.role.name().to_string())
    }
    fn configured(&self) -> bool {
        !self.0.mcp_servers.is_empty()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
}
