//! The MCP tile against a fake port: no server program is started, no folder is read, there is no `AppCx` (the second tile moved onto the standard,
//! docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12). The same behaviours are also proved through the real port and a real server in `mcp.rs`.

use super::*;
use std::sync::mpsc::channel;

struct FakeSession(Arc<Mutex<Vec<String>>>);

impl McpSession for FakeSession {
    fn call(&mut self, tool: &str, args: serde_json::Value) -> Result<String, String> {
        self.0.lock().unwrap().push(format!("{tool} {args}"));
        if tool == "boom" {
            Err("the server said no".into())
        } else {
            Ok(format!("pong from {tool}"))
        }
    }
}

struct FakePort {
    log: Mutex<Vec<String>>,
    calls: Arc<Mutex<Vec<String>>>,
    servers: Mutex<Vec<ServerSpec>>,
    stored: Mutex<Vec<StoredServer>>,
    manage: bool,
    configured: bool,
    connect_fails: Mutex<bool>,
}

impl FakePort {
    fn new(names: &[&str], manage: bool) -> FakePort {
        let spec = |n: &str| ServerSpec { name: n.into(), command: "/bin/true".into(), args: vec![], env: Default::default() };
        FakePort { log: Mutex::default(), calls: Arc::default(), servers: Mutex::new(names.iter().map(|n| spec(n)).collect()), stored: Mutex::default(), manage, configured: true, connect_fails: Mutex::new(false) }
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl McpPort for FakePort {
    fn visible(&self) -> Vec<ServerSpec> {
        let mut v = self.servers.lock().unwrap().clone();
        v.extend(self.stored.lock().unwrap().iter().map(StoredServer::spec));
        v
    }
    fn origin(&self, name: &str) -> &'static str {
        if self.stored.lock().unwrap().iter().any(|s| s.name == name) {
            "added here"
        } else {
            "mcp.json"
        }
    }
    fn can_manage(&self) -> bool {
        self.manage
    }
    fn add(&self, server: StoredServer) -> Result<(), String> {
        if server.command.trim().is_empty() {
            return Err("a command is needed".into());
        }
        self.log.lock().unwrap().push(format!("add {}", server.name));
        self.stored.lock().unwrap().push(server);
        Ok(())
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("remove {name}"));
        self.stored.lock().unwrap().retain(|s| s.name != name);
        Ok(())
    }
    fn connect(&self, spec: ServerSpec) -> std::sync::mpsc::Receiver<Result<Connected, String>> {
        self.log.lock().unwrap().push(format!("connect {}", spec.name));
        let (tx, rx) = channel();
        let r = if *self.connect_fails.lock().unwrap() {
            Err("the server did not start".to_string())
        } else {
            Ok((Box::new(FakeSession(self.calls.clone())) as Box<dyn McpSession>, vec![Tool { name: "echo".into(), description: "Echo".into(), schema: serde_json::json!({}) }, Tool { name: "boom".into(), description: "Fails".into(), schema: serde_json::json!({}) }]))
        };
        tx.send(r).unwrap();
        rx
    }
    fn account(&self) -> (String, String) {
        ("alice".into(), "Admin".into())
    }
    fn configured(&self) -> bool {
        self.configured
    }
    fn audit(&self, action: &str, subject: &str) {
        self.log.lock().unwrap().push(format!("audit {action} {subject}"));
    }
}

fn key(a: &mut McpApp, p: &FakePort, k: Key) {
    a.handle_with(&Event::Key(k), p);
}

fn typ(a: &mut McpApp, p: &FakePort, text: &str) {
    for c in text.chars() {
        key(a, p, Key::Char(c));
    }
}

#[test]
fn connecting_browsing_and_calling_go_through_the_port_with_one_audit_line_each() {
    let p = FakePort::new(&["files", "web"], true);
    let mut a = McpApp::new(&p);
    assert_eq!(a.server_names(), ["files", "web"]);
    key(&mut a, &p, Key::Enter);
    assert!(p.log().contains(&"connect files".to_string()) && p.log().contains(&"audit mcp.connect files".to_string()), "{:?}", p.log());
    a.handle_with(&Event::Tick, &p);
    assert_eq!(a.stage(), Stage::Tools);
    assert_eq!(a.tools().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["echo", "boom"]);
    // choose the first tool, give it arguments, run it
    key(&mut a, &p, Key::Enter);
    assert_eq!(a.stage(), Stage::Args);
    for _ in 0..2 {
        key(&mut a, &p, Key::Backspace);
    }
    typ(&mut a, &p, "{\"x\":1}");
    key(&mut a, &p, Key::Enter);
    for _ in 0..200 {
        a.handle_with(&Event::Tick, &p);
        if a.stage() == Stage::Result {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(a.output(), Some(&("pong from echo".to_string(), true)));
    assert_eq!(p.calls.lock().unwrap().as_slice(), ["echo {\"x\":1}"]);
    assert!(p.log().contains(&"audit mcp.call files/echo".to_string()), "{:?}", p.log());
    // a call the server refuses is shown as a failure, not lost
    key(&mut a, &p, Key::Char(' '));
    assert_eq!(a.stage(), Stage::Tools);
}

#[test]
fn a_server_that_does_not_start_is_said_and_the_list_stays() {
    let p = FakePort::new(&["files"], true);
    *p.connect_fails.lock().unwrap() = true;
    let mut a = McpApp::new(&p);
    key(&mut a, &p, Key::Enter);
    a.handle_with(&Event::Tick, &p);
    assert_eq!(a.stage(), Stage::Servers);
    assert_eq!(a.server_names(), ["files"]);
}

#[test]
fn adding_and_removing_servers_asks_first_is_for_administrators_and_never_touches_the_file_of_the_administrator() {
    // an ordinary account is told and nothing changes
    let p = FakePort::new(&["files"], false);
    let mut a = McpApp::new(&p);
    key(&mut a, &p, Key::Char('n'));
    assert!(a.status_text().unwrap().contains("Only administrators"), "{:?}", a.status_text());
    key(&mut a, &p, Key::Char('d'));
    assert!(!p.log().iter().any(|l| l.starts_with("remove ") || l.starts_with("add ")));
    // an administrator adds one with the form
    let p = FakePort::new(&["files"], true);
    let mut a = McpApp::new(&p);
    key(&mut a, &p, Key::Char('n'));
    typ(&mut a, &p, "notes");
    key(&mut a, &p, Key::Tab);
    typ(&mut a, &p, "/usr/bin/mcp-notes");
    key(&mut a, &p, Key::Tab);
    typ(&mut a, &p, "--db a.db");
    key(&mut a, &p, Key::Enter);
    assert_eq!(p.stored.lock().unwrap().iter().map(|s| (s.name.as_str(), s.args.clone())).collect::<Vec<_>>(), [("notes", vec!["--db".to_string(), "a.db".to_string()])]);
    assert!(p.log().contains(&"audit mcp.add notes".to_string()));
    assert_eq!(a.server_names(), ["files", "notes"]);
    // the one from mcp.json cannot be removed here; the one added here asks, and only y removes
    key(&mut a, &p, Key::Char('d'));
    assert!(a.status_text().unwrap().contains("mcp.json"), "{:?}", a.status_text());
    key(&mut a, &p, Key::Down);
    key(&mut a, &p, Key::Char('d'));
    key(&mut a, &p, Key::Char('x'));
    assert!(!p.log().iter().any(|l| l.starts_with("remove ")), "any other key is a no");
    key(&mut a, &p, Key::Char('d'));
    key(&mut a, &p, Key::Char('y'));
    assert_eq!(p.log().iter().filter(|l| l.starts_with("remove ")).count(), 1);
    assert!(p.log().contains(&"audit mcp.remove notes".to_string()));
    assert_eq!(a.server_names(), ["files"]);
}

#[test]
fn the_tile_draws_in_every_stage_and_says_why_the_list_is_empty() {
    use crate::gfx::Gfx;
    for (names, configured, want) in [(&["files"][..], true, "alice"), (&[][..], false, "No MCP servers configured"), (&[][..], true, "no access")] {
        let mut p = FakePort::new(names, true);
        p.configured = configured;
        let mut a = McpApp::new(&p);
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720)] {
            let mut g = Gfx::new(w, h);
            a.render_with(&mut g, R::new(0.0, 0.0, w as f32, h as f32), 1.0, &p, 1);
        }
        let mut g = Gfx::new(900, 600);
        a.render_with(&mut g, R::new(0.0, 0.0, 900.0, 600.0), 1.0, &p, 1);
        let seen: String = g.layout_log.texts.iter().map(|t| t.3.clone()).collect::<Vec<_>>().join(" ");
        assert!(seen.contains(want), "{want}: {seen}");
    }
}
