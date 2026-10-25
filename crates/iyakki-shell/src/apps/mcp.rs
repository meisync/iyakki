//! MCP: connect to the Model Context Protocol servers your role allows, browse their tools and
//! call them. Servers are host processes configured by an admin; the list shown here is already
//! filtered by the user's policy.

use crate::safeclamp::SafeClamp;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent::mcp::{McpClient, ServerSpec, StoredServer, Tool};

use super::widgets::{render_confirm, Form, FormEvent};
use super::{blink, App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, TextField, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(167, 116, 255);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Servers,
    Connecting,
    Tools,
    Args,
    Calling,
    Result,
}

type Connected = (McpClient, Vec<Tool>);

enum Dialog {
    None,
    Add(Form),
    Remove(String),
}

pub struct McpApp {
    dialog: Dialog,
    status: Option<(String, bool)>,
    servers: Vec<ServerSpec>,
    sel: usize,
    stage: Stage,
    tools: Vec<Tool>,
    tsel: usize,
    client: Option<Arc<Mutex<McpClient>>>,
    connect_rx: Option<Receiver<Result<Connected, String>>>,
    call_rx: Option<Receiver<Result<String, String>>>,
    args: TextField,
    output: Option<(String, bool)>,
    error: Option<String>,
    /// STORE, on this server's page: update, roll back, pin or remove it there.
    request: Option<(String, String)>,
}

impl McpApp {
    pub fn new(cx: &AppCx) -> McpApp {
        let servers = Self::visible(cx);
        McpApp {
            request: None,
            dialog: Dialog::None,
            status: None,
            servers,
            sel: 0,
            stage: Stage::Servers,
            tools: vec![],
            tsel: 0,
            client: None,
            connect_rx: None,
            call_rx: None,
            args: TextField::new("{}", false),
            output: None,
            error: None,
        }
    }

    /// The servers this account may use: the administrator's file and the ones added in this tile.
    fn visible(cx: &AppCx) -> Vec<ServerSpec> {
        let stored = cx.services.mcp.list();
        let mut v: Vec<ServerSpec> = cx.mcp_servers.iter().filter(|s| !stored.iter().any(|x| x.name == s.name)).cloned().collect();
        v.extend(stored.iter().map(StoredServer::spec));
        v.retain(|s| cx.session.policy.allows_mcp(&s.name));
        v
    }

    fn can_manage(cx: &AppCx) -> bool {
        cx.session.policy.manage_settings
    }

    fn add_form() -> Form {
        Form::new("Add an MCP server").text("Name", "", "e.g. files").text("Command", "", "/usr/bin/mcp-files").text("Arguments", "", "optional, separated by spaces")
    }

    /// `sample`, `added here` or `mcp.json`.
    fn origin(cx: &AppCx, name: &str) -> &'static str {
        match cx.services.mcp.list().iter().find(|s| s.name == name) {
            Some(s) if s.sample => "sample",
            Some(_) => "added here",
            None => "mcp.json",
        }
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }
    pub fn output(&self) -> Option<&(String, bool)> {
        self.output.as_ref()
    }
    pub fn server_names(&self) -> Vec<&str> {
        self.servers.iter().map(|s| s.name.as_str()).collect()
    }

    fn ask_remove(&mut self, cx: &AppCx) {
        let Some(sv) = self.servers.get(self.sel) else { return };
        if !Self::can_manage(cx) {
            self.status = Some(("Only administrators can add or remove servers.".into(), false));
        } else if Self::origin(cx, &sv.name) == "mcp.json" {
            self.status = Some((format!("{} comes from the administrator's mcp.json; remove it from that file.", sv.name), false));
        } else {
            self.dialog = Dialog::Remove(sv.name.clone());
        }
    }

    fn connect(&mut self, cx: &AppCx) {
        let Some(spec) = self.servers.get(self.sel).cloned() else { return };
        self.error = None;
        cx.audit.record(&cx.session.user, "mcp.connect", &spec.name);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = McpClient::connect(&spec, Duration::from_secs(10)).and_then(|mut c| {
                let t = c.tools()?;
                Ok((c, t))
            });
            let _ = tx.send(r);
        });
        self.connect_rx = Some(rx);
        self.stage = Stage::Connecting;
    }

    fn call(&mut self, cx: &AppCx) {
        let (Some(client), Some(tool)) = (self.client.clone(), self.tools.get(self.tsel).cloned()) else { return };
        let raw = if self.args.value.trim().is_empty() { "{}".to_string() } else { self.args.value.clone() };
        let args: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                self.error = Some(format!("arguments are not valid JSON: {e}"));
                return;
            }
        };
        self.error = None;
        cx.audit.record(&cx.session.user, "mcp.call", &format!("{}/{}", self.servers[self.sel].name, tool.name));
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = client.lock().map_err(|_| "client lock poisoned".to_string()).and_then(|mut c| c.call(&tool.name, args));
            let _ = tx.send(r);
        });
        self.call_rx = Some(rx);
        self.stage = Stage::Calling;
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.connect_rx {
            if let Ok(r) = rx.try_recv() {
                self.connect_rx = None;
                match r {
                    Ok((c, t)) => {
                        self.client = Some(Arc::new(Mutex::new(c)));
                        self.tools = t;
                        self.tsel = 0;
                        self.stage = Stage::Tools;
                    }
                    Err(e) => {
                        self.error = Some(e);
                        self.stage = Stage::Servers;
                    }
                }
            }
        }
        if let Some(rx) = &self.call_rx {
            if let Ok(r) = rx.try_recv() {
                self.call_rx = None;
                self.output = Some(match r {
                    Ok(t) => (t, true),
                    Err(e) => (e, false),
                });
                self.stage = Stage::Result;
            }
        }
    }
}

impl App for McpApp {
    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    fn title(&self) -> &'static str {
        "MCP"
    }
    fn icon(&self) -> Icon {
        Icon::Mcp
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match self.stage {
            Stage::Servers => "↑↓ choose · Enter connect · N add a server · D remove one · U update, roll back or pin in STORE · Esc back",
            Stage::Connecting | Stage::Calling => "working… · Esc cancel view",
            Stage::Tools => "↑↓ choose · Enter call · Esc disconnect",
            Stage::Args => "JSON arguments · Enter run · Esc back",
            Stage::Result => "any key to continue",
        }
        .into()
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if !matches!(ev, Event::Tick) {
            self.servers = Self::visible(cx);
            self.sel = self.sel.min(self.servers.len().saturating_sub(1));
        }
        match &mut self.dialog {
            Dialog::Add(f) => {
                match f.handle_event(ev) {
                    FormEvent::Cancel => self.dialog = Dialog::None,
                    FormEvent::Submit => {
                        let args: Vec<String> = f.value(2).split_whitespace().map(str::to_string).collect();
                        let s = StoredServer { name: f.value(0), command: f.value(1), args, sample: false };
                        match cx.services.mcp.add(s) {
                            Ok(()) => {
                                cx.audit.record(&cx.session.user, "mcp.add", &f.value(0));
                                self.status = Some((format!("Added {}.", f.value(0)), true));
                                self.dialog = Dialog::None;
                                self.servers = Self::visible(cx);
                            }
                            Err(e) => f.error = Some(e),
                        }
                    }
                    _ => {}
                }
                return Flow::Stay;
            }
            Dialog::Remove(name) => {
                let name = name.clone();
                if matches!(ev, Event::Key(Key::Char('y')) | Event::Key(Key::Enter)) {
                    match cx.services.mcp.remove(&name) {
                        Ok(()) => {
                            cx.audit.record(&cx.session.user, "mcp.remove", &name);
                            self.status = Some((format!("Removed {name}."), true));
                            self.servers.retain(|s| s.name != name);
                            self.sel = self.sel.min(self.servers.len().saturating_sub(1));
                        }
                        Err(e) => self.status = Some((e, false)),
                    }
                    self.dialog = Dialog::None;
                } else if matches!(ev, Event::Key(_)) {
                    self.dialog = Dialog::None;
                }
                return Flow::Stay;
            }
            Dialog::None => {}
        }
        match ev {
            Event::Tab(40) if self.stage == Stage::Servers => {
                if Self::can_manage(cx) {
                    self.dialog = Dialog::Add(Self::add_form());
                } else {
                    self.status = Some(("Only administrators can add or remove servers.".into(), false));
                }
                return Flow::Stay;
            }
            Event::Tab(41) if self.stage == Stage::Servers => {
                self.ask_remove(cx);
                return Flow::Stay;
            }
            Event::Tick => {
                self.poll();
                return Flow::Stay;
            }
            // clicking a row selects it
            Event::Row(i) => {
                match self.stage {
                    Stage::Servers => self.sel = (*i).min(self.servers.len().saturating_sub(1)),
                    Stage::Tools => self.tsel = (*i).min(self.tools.len().saturating_sub(1)),
                    _ => {}
                }
                return Flow::Stay;
            }
            Event::Paste(t) => {
                if self.stage == Stage::Args {
                    self.args.paste(t);
                }
                return Flow::Stay;
            }
            Event::FocusAt(_, x) => {
                if self.stage == Stage::Args {
                    self.args.click(*x);
                }
                return Flow::Stay;
            }
            Event::Scroll(d) => {
                match self.stage {
                    Stage::Servers => self.sel = (self.sel as i32 - d).clamp_safe(0, self.servers.len().saturating_sub(1) as i32) as usize,
                    Stage::Tools => self.tsel = (self.tsel as i32 - d).clamp_safe(0, self.tools.len().saturating_sub(1) as i32) as usize,
                    _ => {}
                }
                return Flow::Stay;
            }
            _ => {}
        }
        let Event::Key(k) = ev else { return Flow::Stay };
        match self.stage {
            Stage::Servers => match k {
                Key::Esc => return Flow::Back,
                Key::Up => self.sel = self.sel.saturating_sub(1),
                Key::Down => self.sel = (self.sel + 1).min(self.servers.len().saturating_sub(1)),
                Key::Enter => self.connect(cx),
                Key::Char('n') => {
                    if Self::can_manage(cx) {
                        self.dialog = Dialog::Add(Self::add_form());
                    } else {
                        self.status = Some(("Only administrators can add or remove servers.".into(), false));
                    }
                }
                Key::Char('d') | Key::Delete => self.ask_remove(cx),
                Key::Char('u') => match self.servers.get(self.sel) {
                    Some(sv) => self.request = Some(("store".into(), format!("installed:mcp:{}", sv.name))),
                    None => self.status = Some(("There is no server to manage.".into(), false)),
                },
                _ => {}
            },
            Stage::Connecting | Stage::Calling => {
                if *k == Key::Esc {
                    self.connect_rx = None;
                    self.call_rx = None;
                    self.stage = if self.client.is_some() { Stage::Tools } else { Stage::Servers };
                }
            }
            Stage::Tools => match k {
                Key::Esc => {
                    self.client = None; // dropping the client stops the server process
                    self.tools.clear();
                    self.stage = Stage::Servers;
                }
                Key::Up => self.tsel = self.tsel.saturating_sub(1),
                Key::Down => self.tsel = (self.tsel + 1).min(self.tools.len().saturating_sub(1)),
                Key::Enter if !self.tools.is_empty() => {
                    self.args.clear();
                    self.stage = Stage::Args;
                }
                _ => {}
            },
            Stage::Args => match k {
                Key::Esc => self.stage = Stage::Tools,
                Key::Enter => self.call(cx),
                other => {
                    self.args.handle(other);
                }
            },
            Stage::Result => self.stage = Stage::Tools,
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 8.0 * s;
        let row = |g: &mut Gfx, idx: usize, y: f32, title: &str, sub: &str, sel: bool| {
            crate::fx::row_at(g, x, y, w, 62.0 * s, 12.0 * s, sel, g.hovered(R::new(x, y, w, 62.0 * s)), ACCENT);
            g.hit(R::new(x, y, w, 62.0 * s), Event::Row(idx), title);
            if sel {
                g.rect(x, y + 10.0 * s, 4.0 * s, 42.0 * s, ACCENT);
            }
            g.text(x + 18.0 * s, y + 9.0 * s, 18.0 * s, TEXT, title, Face::Bold);
            let sub = g.fit(14.0 * s, sub, Face::Sans, w - 36.0 * s);
            g.text(x + 18.0 * s, y + 36.0 * s, 14.0 * s, MUTED, &sub, Face::Sans);
        };
        match self.stage {
            Stage::Servers | Stage::Connecting => {
                g.text(x, y, 15.0 * s, MUTED, &format!("{} server(s) available to {} ({})", self.servers.len(), cx.session.user, cx.session.role.name()), Face::Sans);
                y += 30.0 * s;
                if self.servers.is_empty() {
                    let msg = if cx.mcp_servers.is_empty() { "No MCP servers configured. Add them to /etc/iyakki/mcp.json." } else { "Your role has no access to the configured MCP servers." };
                    crate::fx::empty_state(g, R::new(x, y, w, 240.0 * s), s, Icon::Mcp, "No MCP servers", msg, ACCENT);
                }
                for (i, sv) in self.servers.iter().enumerate() {
                    let tag = Self::origin(cx, &sv.name);
                    row(g, i, y, &sv.name, &format!("{}{} {}", if tag == "mcp.json" { String::new() } else { format!("{} · ", tag.to_uppercase()) }, sv.command, sv.args.join(" ")), i == self.sel);
                    y += 72.0 * s;
                }
                if self.stage == Stage::Connecting {
                    crate::fx::chip(g, x, y + 6.0 * s, s, &format!("connecting{}", ".".repeat(1 + (tick / 10 % 3) as usize)), crate::fx::CYAN, false);
                }
            }
            Stage::Tools | Stage::Args | Stage::Calling | Stage::Result => {
                g.text(x, y, 15.0 * s, MUTED, &format!("{} · {} tool(s)", self.servers.get(self.sel).map_or("", |sv| sv.name.as_str()), self.tools.len()), Face::Sans);
                y += 30.0 * s;
                for (i, t) in self.tools.iter().enumerate().take(5) {
                    row(g, i, y, &t.name, &t.description, i == self.tsel && self.stage == Stage::Tools);
                    y += 72.0 * s;
                }
                if self.stage == Stage::Args {
                    let name = self.tools.get(self.tsel).map_or("", |t| t.name.as_str());
                    g.text(x, y + 4.0 * s, 15.0 * s, MUTED, &format!("Arguments for {name} (JSON object):"), Face::Sans);
                    self.args.render(g, R::new(x, y + 30.0 * s, w, 46.0 * s), true, s, blink(tick), Face::Mono, 0);
                }
                if self.stage == Stage::Calling {
                    crate::fx::chip(g, x, y + 6.0 * s, s, &format!("calling{}", ".".repeat(1 + (tick / 10 % 3) as usize)), crate::fx::CYAN, false);
                }
                if let (Stage::Result, Some((text, ok))) = (self.stage, &self.output) {
                    let col = if *ok { OK } else { ERR };
                    crate::fx::panel(g, R::new(x, y, w, (area.y + area.h - y - 20.0 * s).max(80.0 * s)), s, col);
                    let lh = g.line_height(15.0 * s, Face::Mono);
                    for (i, l) in g.wrap(15.0 * s, text, Face::Mono, w - 28.0 * s).iter().enumerate().take(((area.y + area.h - y - 50.0 * s) / lh).max(1.0) as usize) {
                        g.text(x + 14.0 * s, y + 12.0 * s + i as f32 * lh, 15.0 * s, col, l, Face::Mono);
                    }
                }
            }
        }
        // action buttons (same as the keyboard shortcuts)
        let by = area.y + area.h - 52.0 * s;
        let mut bx = x;
        let mut add = |g: &mut Gfx, label: &str, col: [u8; 4], key: Key| {
            bx += g.button(bx, by, 14.0 * s, label, col, Event::Key(key)) + 10.0 * s;
        };
        match self.stage {
            Stage::Servers => {
                if !self.servers.is_empty() {
                    add(g, "Connect", ACCENT, Key::Enter);
                }
                if Self::can_manage(cx) {
                    bx += g.button(bx, by, 14.0 * s, "Add server (n)", ACCENT, Event::Tab(40)) + 10.0 * s;
                    if !self.servers.is_empty() {
                        g.button(bx, by, 14.0 * s, "Remove (d)", ERR, Event::Tab(41));
                    }
                }
            }
            Stage::Tools if !self.tools.is_empty() => {
                add(g, "Call tool", ACCENT, Key::Enter);
                add(g, "Disconnect", MUTED, Key::Esc);
            }
            Stage::Args => {
                add(g, "Run", ACCENT, Key::Enter);
                add(g, "Back", MUTED, Key::Esc);
            }
            Stage::Result => add(g, "Continue", ACCENT, Key::Enter),
            Stage::Connecting | Stage::Calling => add(g, "Cancel", MUTED, Key::Esc),
            _ => {}
        }
        if let Some(e) = &self.error {
            let e = g.fit(15.0 * s, e, Face::Sans, w);
            g.text(x, by - 26.0 * s, 15.0 * s, ERR, &e, Face::Sans);
        } else if let Some((m, ok)) = &self.status {
            let m = g.fit(14.0 * s, m, Face::Sans, w);
            g.text(x, by - 26.0 * s, 14.0 * s, if *ok { OK } else { ERR }, &m, Face::Sans);
        }
        match &self.dialog {
            Dialog::Add(f) => f.render(g, area, s, ACCENT, blink(tick)),
            Dialog::Remove(n) => render_confirm(g, area, s, &format!("Remove the MCP server “{n}”?"), "Remove"),
            Dialog::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_backend;

    fn wait(app: &mut McpApp, cx: &AppCx, until: impl Fn(&McpApp) -> bool) {
        for _ in 0..600 {
            app.handle(&Event::Tick, cx);
            if until(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out in stage {:?}", app.stage());
    }

    #[test]
    fn connect_browse_call_and_policy_filtering() {
        let (mut cx, d) = cx_with_backend("mcp1", iyakki_agent::chat::Unconfigured);
        let script = d.join("mock.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{}}}\n' "$id";;
    *'"method":"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo"}]}}\n' "$id";;
    *'"method":"tools/call"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id";;
  esac
done
"#,
        )
        .unwrap();
        let spec = |n: &str| ServerSpec { name: n.into(), command: "/bin/sh".into(), args: vec![script.display().to_string()], env: Default::default() };
        cx.mcp_servers = vec![spec("allowed"), spec("secret")];
        cx.session.policy.mcp_servers = vec!["allowed".into()];
        let mut app = McpApp::new(&cx);
        assert_eq!(app.server_names(), vec!["allowed"], "servers outside the policy are hidden");

        app.handle(&Event::Key(Key::Enter), &cx);
        assert_eq!(app.stage(), Stage::Connecting);
        wait(&mut app, &cx, |a| a.stage() == Stage::Tools);
        assert_eq!(app.tools()[0].name, "echo");
        app.handle(&Event::Key(Key::Enter), &cx);
        assert_eq!(app.stage(), Stage::Args);
        for c in "{bad".chars() {
            app.handle(&Event::Key(Key::Char(c)), &cx);
        }
        app.handle(&Event::Key(Key::Enter), &cx);
        assert_eq!(app.stage(), Stage::Args, "invalid JSON is rejected before any call");
        app.args.clear();
        for c in "{\"a\":1}".chars() {
            app.handle(&Event::Key(Key::Char(c)), &cx);
        }
        app.handle(&Event::Key(Key::Enter), &cx);
        wait(&mut app, &cx, |a| a.stage() == Stage::Result);
        assert_eq!(app.output(), Some(&("pong".to_string(), true)));
        let mut g = Gfx::new(900, 600);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 80.0, 900.0, 520.0), 1.0, &cx, 2);
        app.handle(&Event::Key(Key::Char('x')), &cx);
        assert_eq!(app.stage(), Stage::Tools);
        app.handle(&Event::Key(Key::Esc), &cx);
        assert_eq!(app.stage(), Stage::Servers);
        assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    }

    #[test]
    fn administrators_add_and_remove_servers_here_samples_are_marked_and_file_servers_are_left_alone() {
        let (mut cx, d) = cx_with_backend("mcp3", iyakki_agent::chat::Unconfigured);
        let script = d.join("mock.sh");
        std::fs::write(&script, "#!/bin/sh\ncat >/dev/null\n").unwrap();
        cx.mcp_servers = vec![ServerSpec { name: "from-file".into(), command: "/bin/sh".into(), args: vec![], env: Default::default() }];
        cx.session.policy.mcp_servers = vec!["*".into()];
        // a sample put there by IYAKKI
        cx.services.mcp.add(StoredServer { name: "iyakki-demo".into(), command: "/bin/sh".into(), args: vec![], sample: true }).unwrap();
        let mut app = McpApp::new(&cx);
        assert_eq!(app.server_names(), vec!["from-file", "iyakki-demo"]);
        let mut g = Gfx::new(1000, 650);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 46.0, 1000.0, 604.0), 1.0, &cx, 0);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(40)) && g.hits.iter().any(|h| h.ev == Event::Tab(41)));
        assert_eq!(McpApp::origin(&cx, "iyakki-demo"), "sample");
        assert_eq!(McpApp::origin(&cx, "from-file"), "mcp.json");
        // the file's server cannot be removed from here
        app.handle(&Event::Key(Key::Char('d')), &cx);
        assert!(matches!(app.dialog, Dialog::None) && app.status.as_ref().unwrap().0.contains("mcp.json"));
        // add one with the form
        app.handle(&Event::Key(Key::Char('n')), &cx);
        for c in "files".chars() {
            app.handle(&Event::Key(Key::Char(c)), &cx);
        }
        app.handle(&Event::Key(Key::Tab), &cx);
        for c in format!("/bin/sh {}", script.display()).chars() {
            app.handle(&Event::Key(Key::Char(c)), &cx);
        }
        app.handle(&Event::Key(Key::Enter), &cx);
        for _ in 0..3 {
            if !matches!(app.dialog, Dialog::None) {
                app.handle(&Event::Key(Key::Enter), &cx);
            }
        }
        assert!(app.server_names().contains(&"files"), "{:?} {:?}", app.server_names(), app.status);
        let stored = cx.services.mcp.list();
        assert_eq!(stored.iter().find(|s| s.name == "files").map(|s| s.args.len()), Some(0), "the whole command went in the command field");
        // remove the sample after asking
        app.handle(&Event::Key(Key::Down), &cx);
        assert_eq!(app.servers[app.sel].name, "iyakki-demo");
        app.handle(&Event::Key(Key::Char('d')), &cx);
        assert!(matches!(app.dialog, Dialog::Remove(_)));
        app.render(&mut g, R::new(0.0, 46.0, 1000.0, 604.0), 1.0, &cx, 0);
        app.handle(&Event::Key(Key::Char('n')), &cx); // anything but y keeps it
        assert!(app.server_names().contains(&"iyakki-demo"));
        app.handle(&Event::Key(Key::Char('d')), &cx);
        app.handle(&Event::Key(Key::Char('y')), &cx);
        assert!(!app.server_names().contains(&"iyakki-demo") && cx.services.mcp.list().iter().all(|s| s.name != "iyakki-demo"));
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("mcp.add") && log.contains("mcp.remove"), "{log}");
        // a bad name stays in the form with the reason
        app.handle(&Event::Key(Key::Char('n')), &cx);
        for c in "bad name".chars() {
            app.handle(&Event::Key(Key::Char(c)), &cx);
        }
        app.handle(&Event::Key(Key::Tab), &cx);
        app.handle(&Event::Key(Key::Char('x')), &cx);
        for _ in 0..3 {
            if matches!(app.dialog, Dialog::Add(_)) {
                app.handle(&Event::Key(Key::Enter), &cx);
            }
        }
        match &app.dialog {
            Dialog::Add(f) => assert!(f.error.as_deref().unwrap_or("").contains("name")),
            _ => panic!("the form should stay open"),
        }
        app.handle(&Event::Key(Key::Esc), &cx);
        // someone who is not an administrator cannot
        cx.session.policy.manage_settings = false;
        let mut app = McpApp::new(&cx);
        app.handle(&Event::Key(Key::Char('n')), &cx);
        assert!(matches!(app.dialog, Dialog::None) && app.status.as_ref().unwrap().0.contains("Only administrators"));
    }

    #[test]
    fn failed_connection_returns_to_list_with_error() {
        let (mut cx, _d) = cx_with_backend("mcp2", iyakki_agent::chat::Unconfigured);
        cx.mcp_servers = vec![ServerSpec { name: "bad".into(), command: "/no/such".into(), args: vec![], env: Default::default() }];
        cx.session.policy.mcp_servers = vec![];
        let mut app = McpApp::new(&cx);
        assert!(app.server_names().is_empty(), "a role with no MCP access sees no servers");
        cx.session.policy.mcp_servers = vec!["*".into()];
        let mut app2 = McpApp::new(&cx);
        app2.handle(&Event::Key(Key::Enter), &cx);
        wait(&mut app2, &cx, |a| a.stage() == Stage::Servers);
        assert!(app2.error.as_deref().unwrap().contains("cannot start"));
        app.handle(&Event::Key(Key::Enter), &cx); // nothing to connect to: must not panic
    }
}
