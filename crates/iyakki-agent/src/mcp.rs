//! MCP: a Model Context Protocol client over stdio (newline-delimited JSON-RPC 2.0).
//!
//! Servers are *host processes* started from `/etc/iyakki/mcp.json` (admin-controlled), not sandboxed
//! guests, so which users may use which server is decided by the user's policy (`allows_mcp`).
//!
//! ```json
//! {"servers":[{"name":"files","command":"/usr/bin/mcp-files","args":["--root","/shared"],"env":{}}]}
//! ```

use std::io::{BufRead, Write};

use serde_json::{json, Value};
pub use iyakki_shared::mcp::{ConfigFile, McpClient, load_config, parse_config};
pub use iyakki_shared::mcp::{ServerSpec, StoredServer, Tool};










// ------------------------------------------------------------------------------------------ the sample server

/// A small MCP server for trying the protocol: `echo`, `add`, `now` and `dice`. IYAKKI's own programs start it when run
/// with `--mcp-demo` (see [`demo_spec`]); it reads requests from `input` and writes answers to `output`.
pub fn demo_serve(input: impl BufRead, mut output: impl Write) {
    let tool = |name: &str, desc: &str, props: Value| json!({"name": name, "description": desc, "inputSchema": {"type": "object", "properties": props}});
    let tools = json!([
        tool("echo", "Return the text you give it.", json!({"text": {"type": "string"}})),
        tool("add", "Add two numbers.", json!({"a": {"type": "number"}, "b": {"type": "number"}})),
        tool("now", "The current date and time on this machine.", json!({})),
        tool("dice", "Roll a die (6 sides unless `sides` is given).", json!({"sides": {"type": "integer"}})),
    ]);
    for line in input.lines().map_while(Result::ok) {
        let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = req.get("id").cloned() else { continue }; // notifications get no answer
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let args = req.pointer("/params/arguments").cloned().unwrap_or(json!({}));
        let text = |t: String| json!({"content": [{"type": "text", "text": t}]});
        let fail = |t: &str| json!({"isError": true, "content": [{"type": "text", "text": t}]});
        let result = match method {
            "initialize" => json!({"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "iyakki-demo", "version": env!("CARGO_PKG_VERSION")}}),
            "tools/list" => json!({"tools": tools}),
            "ping" => json!({}),
            "tools/call" => match req.pointer("/params/name").and_then(Value::as_str).unwrap_or("") {
                "echo" => text(args.get("text").and_then(Value::as_str).unwrap_or("").to_string()),
                "add" => match (args.get("a").and_then(Value::as_f64), args.get("b").and_then(Value::as_f64)) {
                    (Some(a), Some(b)) => text(format!("{}", a + b)),
                    _ => fail("give numbers a and b"),
                },
                "now" => text(crate::clock::describe(&crate::clock::Zone::system(), crate::clock::now_unix())),
                "dice" => {
                    let sides = args.get("sides").and_then(Value::as_u64).unwrap_or(6).clamp(2, 1000) as u32;
                    let mut r = [0u8; 4];
                    let _ = iyakki_sec::random_fill(&mut r);
                    text(format!("{}", u32::from_le_bytes(r) % sides + 1))
                }
                other => fail(&format!("no tool called {other}")),
            },
            other => {
                let _ = writeln!(output, "{}", json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("unknown method {other}")}}));
                let _ = output.flush();
                continue;
            }
        };
        let _ = writeln!(output, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result}));
        let _ = output.flush();
    }
}

/// Runs the sample server on this process's standard input and output.
pub fn demo_serve_stdio() {
    demo_serve(std::io::BufReader::new(std::io::stdin()), std::io::stdout());
}

/// The sample server as a server to connect to: this program again, with `--mcp-demo`.
pub fn demo_spec() -> ServerSpec {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "iyakki".into());
    ServerSpec { name: "iyakki-demo".into(), command: exe, args: vec!["--mcp-demo".into()], env: Default::default() }
}

// ------------------------------------------------------------------------------------------- servers added here



/// MCP servers added from the MCP tile (the administrator's `mcp.json` is separate and is not changed from here).
/// The servers an account may use: the administrator's `mcp.json` ones that are not shadowed by a stored one, then the stored ones, kept to what the
/// account's policy allows. The MCP tile and the assistant's `mcp_servers` tool both ask this, so they cannot disagree.
pub fn visible_servers(file: &[ServerSpec], stored: &[StoredServer], policy: &iyakki_sec::policy::Policy) -> Vec<ServerSpec> {
    let mut v: Vec<ServerSpec> = file.iter().filter(|s| !stored.iter().any(|x| x.name == s.name)).cloned().collect();
    v.extend(stored.iter().map(StoredServer::spec));
    v.retain(|s| policy.allows_mcp(&s.name));
    v
}

/// `sample`, `added here` or `mcp.json`.
pub fn origin_of(stored: &[StoredServer], name: &str) -> &'static str {
    match stored.iter().find(|s| s.name == name) {
        Some(s) if s.sample => "sample",
        Some(_) => "added here",
        None => "mcp.json",
    }
}

pub struct McpStore {
    path: std::path::PathBuf,
    lock: std::sync::Mutex<()>,
}

pub fn valid_server_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 32 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl McpStore {
    pub fn new(path: &std::path::Path) -> McpStore {
        McpStore { path: path.into(), lock: std::sync::Mutex::new(()) }
    }

    fn load(&self) -> Vec<StoredServer> {
        std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, v: &[StoredServer]) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(p) = self.path.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("tmp");
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(&serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn list(&self) -> Vec<StoredServer> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load()
    }

    pub fn add(&self, s: StoredServer) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if !valid_server_name(&s.name) {
            return Err("the name: letters, digits, - and _ (up to 32)".into());
        }
        if s.command.trim().is_empty() || s.command.len() > 500 || s.args.len() > 32 || s.args.iter().any(|a| a.len() > 500) {
            return Err("the command is empty or too long".into());
        }
        let mut v = self.load();
        if v.iter().any(|x| x.name == s.name) {
            return Err(format!("a server called {} is already there", s.name));
        }
        v.push(s);
        self.save(&v)
    }

    /// Put `s` in place of the server of the same name (or add it), in one write, so an update never leaves the server missing.
    /// Returns the definition that was replaced.
    pub fn replace(&self, s: StoredServer) -> Result<Option<StoredServer>, String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if !valid_server_name(&s.name) {
            return Err("the name: letters, digits, - and _ (up to 32)".into());
        }
        if s.command.trim().is_empty() || s.command.len() > 500 || s.args.len() > 32 || s.args.iter().any(|a| a.len() > 500) {
            return Err("the command is empty or too long".into());
        }
        let mut v = self.load();
        let old = v.iter().position(|x| x.name == s.name).map(|i| v.remove(i));
        v.push(s);
        self.save(&v)?;
        Ok(old)
    }

    pub fn remove(&self, name: &str) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = self.load();
        let n = v.len();
        v.retain(|x| x.name != name);
        if v.len() == n {
            return Err(format!("no server called {name} was added here (servers from mcp.json are changed in that file)"));
        }
        self.save(&v)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny MCP server in POSIX sh, enough to exercise the protocol.
    pub fn mock_server(dir: &std::path::Path) -> ServerSpec {
        let script = dir.join("mock-mcp.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"mock","version":"1"}}}\n' "$id";;
    *'"method":"tools/list"'*) echo 'not json: a log line'; printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo text","inputSchema":{"type":"object"}},{"name":"boom","description":"fails"}]}}\n' "$id";;
    *'"name":"boom"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"isError":true,"content":[{"type":"text","text":"nope"}]}}\n' "$id";;
    *'"method":"tools/call"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"called"},{"type":"text","text":"twice"}]}}\n' "$id";;
    *'"method":"slow"'*) ;;
  esac
done
"#,
        )
        .unwrap();
        ServerSpec { name: "mock".into(), command: "/bin/sh".into(), args: vec![script.display().to_string()], env: Default::default() }
    }

    #[test]
    fn config_parsing() {
        let s = parse_config(r#"{"servers":[{"name":"a","command":"/x"},{"name":"b","command":"/y","args":["1"],"env":{"K":"V"}}]}"#).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].env["K"], "V");
        assert!(parse_config(r#"{"servers":[{"name":"a","command":"/x"},{"name":"a","command":"/y"}]}"#).is_err());
        assert!(parse_config("not json").is_err());
        assert!(parse_config("{}").unwrap().is_empty());
    }

    #[test]
    fn handshake_list_and_call() {
        let d = std::env::temp_dir().join(format!("iyakki-mcp-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let mut c = McpClient::connect(&mock_server(&d), Duration::from_secs(5)).unwrap();
        let tools = c.tools().unwrap();
        assert_eq!(tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["echo", "boom"]);
        assert_eq!(tools[0].description, "Echo text");
        assert_eq!(c.call("echo", json!({"text": "hi"})).unwrap(), "called\ntwice");
        assert!(c.call("boom", json!({})).unwrap_err().contains("nope"));
        assert!(c.call("echo", json!("not an object")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_sample_server_answers_the_protocol_and_its_four_tools() {
        let reqs = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "echo", "arguments": {"text": "hello"}}}),
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "add", "arguments": {"a": 2, "b": 3.5}}}),
            json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "add", "arguments": {"a": "x"}}}),
            json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "dice", "arguments": {"sides": 6}}}),
            json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "now", "arguments": {}}}),
            json!({"jsonrpc": "2.0", "id": 8, "method": "tools/call", "params": {"name": "nope"}}),
            json!({"jsonrpc": "2.0", "id": 9, "method": "weird"}),
        ];
        let input: String = reqs.iter().map(|r| format!("{r}\n")).chain(std::iter::once("not json\n".to_string())).collect();
        let mut out = Vec::new();
        demo_serve(std::io::Cursor::new(input), &mut out);
        let replies: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(replies.len(), 9, "notifications and junk get no answer");
        assert_eq!(replies[0]["result"]["serverInfo"]["name"], "iyakki-demo");
        let names: Vec<&str> = replies[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["echo", "add", "now", "dice"]);
        assert_eq!(replies[2]["result"]["content"][0]["text"], "hello");
        assert_eq!(replies[3]["result"]["content"][0]["text"], "5.5");
        assert_eq!(replies[4]["result"]["isError"], true);
        let die: u32 = replies[5]["result"]["content"][0]["text"].as_str().unwrap().parse().unwrap();
        assert!((1..=6).contains(&die));
        assert!(replies[6]["result"]["content"][0]["text"].as_str().unwrap().contains("202"));
        assert_eq!(replies[7]["result"]["isError"], true);
        assert_eq!(replies[8]["error"]["code"], -32601);
    }

    #[test]
    fn servers_added_here_are_kept_checked_and_removed() {
        let d = std::env::temp_dir().join(format!("iyakki-mcpstore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let s = McpStore::new(&d.join("mcp-servers.json"));
        assert!(s.list().is_empty());
        let one = StoredServer { name: "files".into(), command: "/usr/bin/mcp-files".into(), args: vec!["--root".into(), "/shared".into()], sample: false };
        s.add(one.clone()).unwrap();
        s.add(StoredServer { sample: true, ..demo_spec_as_stored() }).unwrap();
        assert_eq!(s.list().len(), 2);
        assert!(s.add(one.clone()).unwrap_err().contains("already there"));
        for bad in ["", "has space", "../x", &"x".repeat(33)] {
            assert!(s.add(StoredServer { name: bad.into(), ..one.clone() }).is_err(), "{bad:?}");
        }
        assert!(s.add(StoredServer { name: "e".into(), command: "  ".into(), ..one.clone() }).is_err());
        assert_eq!(s.list()[0].spec().command, "/usr/bin/mcp-files");
        s.remove("files").unwrap();
        assert!(s.remove("files").unwrap_err().contains("no server called"));
        assert_eq!(s.list().len(), 1);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(d.join("mcp-servers.json")).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&d);
    }

    fn demo_spec_as_stored() -> StoredServer {
        let d = demo_spec();
        StoredServer { name: d.name, command: d.command, args: d.args, sample: true }
    }

    #[test]
    fn missing_binary_and_timeouts_are_reported() {
        let bad = ServerSpec { name: "x".into(), command: "/no/such/binary".into(), args: vec![], env: Default::default() };
        assert!(McpClient::connect(&bad, Duration::from_secs(1)).unwrap_err().contains("cannot start"));
        // a server that never answers initialize
        let silent = ServerSpec { name: "s".into(), command: "/bin/sh".into(), args: vec!["-c".into(), "cat >/dev/null".into()], env: Default::default() };
        let t = std::time::Instant::now();
        assert!(McpClient::connect(&silent, Duration::from_millis(300)).unwrap_err().contains("timed out"));
        assert!(t.elapsed() < Duration::from_secs(3));
    }
}

#[cfg(test)]
use std::time::Duration;
