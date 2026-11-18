//! Types moved out of `iyakki_agent::mcp` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::collections::BTreeMap;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ServerSpec {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct StoredServer {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Put there by IYAKKI as an example, to be deleted when it is not wanted.
    #[serde(default)]
    pub sample: bool,
}

impl StoredServer {
    pub fn spec(&self) -> ServerSpec {
        ServerSpec { name: self.name.clone(), command: self.command.clone(), args: self.args.clone(), env: Default::default() }
    }
}

#[derive(Deserialize, Default)]
pub struct ConfigFile {
    #[serde(default)]
    pub servers: Vec<ServerSpec>,
}

pub fn parse_config(json: &str) -> Result<Vec<ServerSpec>, String> {
    let f: ConfigFile = serde_json::from_str(json).map_err(|e| format!("invalid MCP config: {e}"))?;
    let mut seen = std::collections::HashSet::new();
    for s in &f.servers {
        if s.name.is_empty() || !seen.insert(s.name.clone()) {
            return Err(format!("duplicate or empty server name `{}`", s.name));
        }
    }
    Ok(f.servers)
}

pub fn load_config(path: &str) -> Vec<ServerSpec> {
    std::fs::read_to_string(path).ok().and_then(|t| parse_config(&t).ok()).unwrap_or_default()
}

pub struct McpClient {
    pub child: Child,
    pub stdin: ChildStdin,
    pub rx: Receiver<String>,
    pub next_id: u64,
    pub timeout: Duration,
    pub server_name: String,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "McpClient({})", self.server_name)
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl McpClient {
    /// Start the server and perform the MCP `initialize` handshake.
    pub fn connect(spec: &ServerSpec, timeout: Duration) -> Result<McpClient, String> {
        let mut child = Command::new(&spec.command)
            .args(&spec.args)
            .envs(&spec.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start `{}`: {e}", spec.command))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut c = McpClient { child, stdin, rx, next_id: 1, timeout, server_name: spec.name.clone() };
        c.request(
            "initialize",
            json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "iyakki", "version": env!("CARGO_PKG_VERSION")}}),
        )?;
        c.notify("notifications/initialized")?;
        Ok(c)
    }

    pub fn send(&mut self, v: &Value) -> Result<(), String> {
        writeln!(self.stdin, "{v}").and_then(|_| self.stdin.flush()).map_err(|e| format!("server closed the connection: {e}"))
    }

    pub fn notify(&mut self, method: &str) -> Result<(), String> {
        self.send(&json!({"jsonrpc": "2.0", "method": method}))
    }

    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let line = self.rx.recv_timeout(left).map_err(|_| format!("`{method}` timed out or the server exited"))?;
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue }; // ignore non-JSON log lines
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue; // notification or stale response
            }
            if let Some(e) = v.get("error") {
                return Err(format!("server error: {}", e.get("message").and_then(Value::as_str).unwrap_or("unknown")));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    pub fn tools(&mut self) -> Result<Vec<Tool>, String> {
        let r = self.request("tools/list", json!({}))?;
        Ok(r.get("tools")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        Some(Tool {
                            name: t.get("name")?.as_str()?.to_string(),
                            description: t.get("description").and_then(Value::as_str).unwrap_or("").to_string(),
                            schema: t.get("inputSchema").cloned().unwrap_or(Value::Null),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Call a tool; returns the concatenated text content. `args` must be a JSON object.
    pub fn call(&mut self, tool: &str, args: Value) -> Result<String, String> {
        if !args.is_object() {
            return Err("tool arguments must be a JSON object".into());
        }
        let r = self.request("tools/call", json!({"name": tool, "arguments": args}))?;
        let text: Vec<&str> = r
            .get("content")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|c| c.get("text").and_then(Value::as_str)).collect())
            .unwrap_or_default();
        let out = text.join("\n");
        if r.get("isError").and_then(Value::as_bool) == Some(true) {
            Err(format!("tool reported an error: {out}"))
        } else {
            Ok(out)
        }
    }
}
