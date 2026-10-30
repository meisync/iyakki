//! The lead assistant's tools for SSH tunnels (see `docs/SSH-TUNNELS.md`): `ssh_tunnel_list` reads (no card); `ssh_tunnel_add`,
//! `ssh_tunnel_edit`, `ssh_tunnel_enable`, `ssh_tunnel_disable` and `ssh_tunnel_delete` each ask on a card that says in plain words what
//! will change, and are on the hard-ask list (`agent::hard_ask`): they ask every time, in Auto mode too, and "Allow for this session"
//! does not remember them.
//!
//! * The assistant never sees, sends or hears a key or password: a key is a NAME of the person's Git and SSH keys (vault entry
//!   `ssh.<name>`); text that looks like key material is refused without being echoed.
//! * A new host key is never trusted by a tool: the result shows the fingerprint and says the person must confirm it in Settings ->
//!   Network -> SSH tunnels. A changed key is reported as a refusal.
//! * The checks are the manager's own ([`crate::sshtunnel`]); there is no second path around them.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::gitkeys::HostCheck;
use crate::sshtunnel::{Hop, Kind, Tunnel};
use crate::sshtunnel_mgr::{HostReport, Manager};

pub const PROMPT_RULE: &str = "\nSSH TUNNELS: ssh_tunnel_list shows the person's tunnels (pages behind an SSH host, reachable on 127.0.0.1). To add, edit, enable, disable or delete one, call the matching ssh_tunnel_* tool: each asks the person every time. Refer to the key by its NAME only; if there is none, ask the person to add it in Settings -> Git and SSH keys. You cannot trust a new host key: show the fingerprint and tell the person to confirm it in Settings -> Network -> SSH tunnels.";

pub const WRITE_TOOLS: [&str; 5] = ["ssh_tunnel_add", "ssh_tunnel_edit", "ssh_tunnel_enable", "ssh_tunnel_disable", "ssh_tunnel_delete"];
pub const READ_TOOL: &str = "ssh_tunnel_list";

pub fn is_tunnel_tool(n: &str) -> bool {
    n == READ_TOOL || WRITE_TOOLS.contains(&n)
}

pub struct SshTunnelTools {
    inner: Arc<dyn Tools>,
    mgr: Arc<Manager>,
}

impl SshTunnelTools {
    pub fn new(inner: Arc<dyn Tools>, mgr: Arc<Manager>) -> SshTunnelTools {
        SshTunnelTools { inner, mgr }
    }

    fn find(&self, args: &Value) -> Result<Tunnel, String> {
        let w = text(args, "tunnel");
        if w.is_empty() {
            return Err("name the tunnel (its id or its name; ssh_tunnel_list shows them)".into());
        }
        self.mgr.tunnels().into_iter().find(|t| t.id == w || t.name.eq_ignore_ascii_case(&w)).ok_or_else(|| "there is no such tunnel (ssh_tunnel_list shows them)".into())
    }

    fn key_exists(&self, key: &str) -> Result<(), String> {
        if !crate::gitkeys::valid_key_id(key) {
            return Err("the key is the NAME of a key in Settings -> Git and SSH keys (letters, digits, `.`, `_`, `-`); never send key text".into());
        }
        if self.mgr.keys().key(key).is_none() {
            return Err(format!("there is no key called {key} in the person's vault. Ask the person to add it in Settings -> Git and SSH keys (make or import it); do not ask for the key in the chat."));
        }
        Ok(())
    }

    /// The tunnel a call asks for: all fields for an add, the old tunnel with the given fields replaced for an edit.
    fn wanted(&self, name: &str, args: &Value) -> Result<Tunnel, String> {
        reject_secrets(args)?;
        let old = if name == "ssh_tunnel_edit" { Some(self.find(args)?) } else { None };
        let mut t = old.clone().unwrap_or_else(|| Tunnel { port: 22, ..Default::default() });
        let given = |k: &str| args.get(k).is_some_and(|v| !v.is_null());
        if given("name") {
            t.name = text(args, "name");
        }
        if given("host") {
            t.host = text(args, "host");
        }
        if given("port") {
            t.port = num(args, "port")?;
        }
        if given("user") {
            t.user = text(args, "user");
        }
        if given("key") {
            t.key = text(args, "key");
            t.own_key = old.as_ref().is_some_and(|o| o.own_key && o.key == t.key);
        }
        if given("jump") {
            t.jump = parse_jumps(args.get("jump"), &t.user)?;
        }
        if given("kind") {
            t.kind = match text(args, "kind").as_str() {
                "local" | "local_forward" | "page" => Kind::LocalForward,
                "socks" | "dynamic_socks" => Kind::DynamicSocks,
                _ => return Err("kind is `local` (one page) or `socks` (any site)".into()),
            };
        }
        if given("local_port") {
            t.local_port = num(args, "local_port")?;
        }
        if given("target_host") {
            t.target_host = text(args, "target_host");
        }
        if given("target_port") {
            t.target_port = num(args, "target_port")?;
        }
        if t.kind == Kind::DynamicSocks {
            t.target_host.clear();
            t.target_port = 0;
        }
        t.validate_fields()?;
        self.key_exists(&t.key)?;
        Ok(t)
    }

    /// What the hosts of the route present. A new key is never trusted here.
    fn host_notes(&self, t: &Tunnel) -> (bool, Vec<String>) {
        let mut all = true;
        let mut out = vec![];
        for h in self.mgr.check_hosts(t) {
            let ok = matches!(h.check, Ok(HostCheck::Trusted));
            all &= ok;
            out.push(match &h.check {
                Ok(HostCheck::New(_)) => format!("{} The person must confirm that fingerprint in Settings -> Network -> SSH tunnels (Test); you cannot trust a host key.", note_text(&h)),
                _ => note_text(&h),
            });
        }
        (all, out)
    }

    fn card(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "ssh_tunnel_add" | "ssh_tunnel_edit" => {
                let t = self.wanted(name, args)?;
                let head = if name == "ssh_tunnel_add" { format!("Add the SSH tunnel “{}” (it starts disabled).", t.name) } else { format!("Change the SSH tunnel “{}”; it will become:", self.find(args).map(|o| o.name).unwrap_or_default()) };
                Ok(format!("{head}\n{}", profile_lines(&t)))
            }
            "ssh_tunnel_enable" => {
                let t = self.find(args)?;
                Ok(format!("Enable the SSH tunnel “{}”: it starts now, with this device, and reconnects by itself.\n{}", t.name, profile_lines(&t)))
            }
            "ssh_tunnel_disable" => {
                let t = self.find(args)?;
                Ok(format!("Disable the SSH tunnel “{}”: it stops now and does not start again until enabled. The tunnel is kept.", t.name))
            }
            "ssh_tunnel_delete" => {
                let t = self.find(args)?;
                Ok(format!(
                    "DELETE the SSH tunnel “{}”. It stops now and is removed. {}The trusted host key of {} is removed from your known hosts unless another tunnel uses that host. This cannot be undone.\n{}",
                    t.name,
                    if t.own_key { format!("Its key made for it (vault entry ssh.{}) is deleted from your vault. ", t.key) } else { format!("The key {} (vault entry ssh.{}) was yours before and stays in your vault. ", t.key, t.key) },
                    t.hops().iter().map(|(h, p)| crate::ssh::pattern(h, *p)).collect::<Vec<_>>().join(", "),
                    profile_lines(&t)
                ))
            }
            _ => Err("not a tunnel tool".into()),
        }
    }
}

fn note_text(h: &HostReport) -> String {
    h.text()
}

fn text(args: &Value, k: &str) -> String {
    args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

fn num(args: &Value, k: &str) -> Result<u16, String> {
    let v = args.get(k).ok_or_else(|| format!("{k} is missing"))?;
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).and_then(|n| u16::try_from(n).ok()).ok_or_else(|| format!("{k} is a number from 1 to 65535"))
}

/// Text that looks like a private key or a token is refused, and never repeated.
fn reject_secrets(args: &Value) -> Result<(), String> {
    fn walk(v: &Value) -> bool {
        match v {
            Value::String(s) => s.contains("-----BEGIN") || s.contains("PRIVATE KEY") || s.contains('\n') || s.len() > 300,
            Value::Array(a) => a.iter().any(walk),
            Value::Object(o) => o.values().any(walk),
            _ => false,
        }
    }
    if walk(args) {
        return Err("a value looks like key text or is not a plain short setting. Never send keys or passwords: use the NAME of a key from Settings -> Git and SSH keys.".into());
    }
    Ok(())
}

fn parse_jumps(v: Option<&Value>, default_user: &str) -> Result<Vec<Hop>, String> {
    let Some(a) = v.and_then(Value::as_array) else { return Err("jump is a list of `user@host:port` words".into()) };
    let mut out = vec![];
    for w in a.iter().filter_map(Value::as_str).map(str::trim).filter(|w| !w.is_empty()) {
        let (user, rest) = w.split_once('@').map_or((default_user, w), |(u, r)| (u, r));
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().map_err(|_| "a jump host port is a number".to_string())?),
            None => (rest, 22),
        };
        out.push(Hop { host: host.to_string(), port, user: user.to_string() });
    }
    Ok(out)
}

/// The words of the card: every field that matters, and where the key lives.
fn profile_lines(t: &Tunnel) -> String {
    let kind = match t.kind {
        Kind::LocalForward => format!("one page: {}:{} as the SSH host sees it", t.target_host, t.target_port),
        Kind::DynamicSocks => "any site the SSH host's network reaches (a SOCKS proxy)".to_string(),
    };
    let jumps = if t.jump.is_empty() { "none".to_string() } else { t.jump.iter().map(|h| format!("{}@{}:{}", h.user, h.host, h.port)).collect::<Vec<_>>().join(" then ") };
    format!(
        "• SSH host: {}:{}, user {}\n• Jump hosts before it: {jumps}\n• Reaches: {kind}\n• Listens on this device only: 127.0.0.1:{}\n• Key: vault entry ssh.{} (the assistant never sees it)",
        t.host,
        t.port,
        t.user,
        if t.local_port == 0 { "(a free port is chosen)".to_string() } else { t.local_port.to_string() },
        t.key
    )
}

fn one_line(t: &Tunnel, st: &crate::sshtunnel_mgr::Status, now: u64) -> String {
    format!(
        "{} · {} · {} · {} · 127.0.0.1:{} · {} · key {} · {}{}",
        t.id,
        t.name,
        t.route_text(),
        match t.kind { Kind::LocalForward => format!("page {}:{}", t.target_host, t.target_port), Kind::DynamicSocks => "SOCKS".into() },
        t.local_port,
        if t.enabled { "enabled" } else { "disabled" },
        t.key,
        st.line(t.enabled, now),
        st.error.as_ref().map(|e| format!(" · {e}")).unwrap_or_default()
    )
}

impl Tools for SshTunnelTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_tunnel_tool(&d.spec.name));
        let fields = json!({
            "name": {"type": "string", "description": "Short name of the tunnel."},
            "host": {"type": "string", "description": "The front-facing SSH host (name or IPv4)."},
            "port": {"type": "integer", "description": "Its SSH port (22)."},
            "user": {"type": "string", "description": "The user on that host."},
            "key": {"type": "string", "description": "The NAME of a key in Settings -> Git and SSH keys (vault entry ssh.<name>). Never key text."},
            "jump": {"type": "array", "items": {"type": "string"}, "description": "Hosts before it, as user@host:port."},
            "kind": {"type": "string", "enum": ["local", "socks"], "description": "local: one page behind the host; socks: any site its network reaches."},
            "local_port": {"type": "integer", "description": "Port on this device (1024 or above); leave out for a free one."},
            "target_host": {"type": "string", "description": "local only: the page's host as the SSH host sees it."},
            "target_port": {"type": "integer", "description": "local only: the page's port."}
        });
        let with_tunnel = |req: &[&str], all: bool| {
            let mut p = if all { fields.as_object().cloned().unwrap_or_default() } else { serde_json::Map::new() };
            p.insert("tunnel".into(), json!({"type": "string", "description": "The tunnel's id or name."}));
            json!({"type": "object", "properties": p, "required": req})
        };
        let ask = |name: &str, desc: &str, schema: Value| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk: Risk::Ask, untrusted: false };
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "List the person's SSH tunnels with their state (no secrets).".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ask("ssh_tunnel_add", "Add an SSH tunnel (starts disabled). The person approves a card with every field. The key is a NAME; a new host key is never trusted by this tool, the result shows its fingerprint for the person to confirm in Settings.", json!({"type": "object", "properties": fields, "required": ["name", "host", "user", "key"]})));
        v.push(ask("ssh_tunnel_edit", "Change fields of an SSH tunnel (only the ones you give). The person approves a card showing the result.", with_tunnel(&["tunnel"], true)));
        v.push(ask("ssh_tunnel_enable", "Enable a tunnel so it starts now and with the device. Refused while a host of its route is not trusted (the result shows the fingerprint for the person).", with_tunnel(&["tunnel"], false)));
        v.push(ask("ssh_tunnel_disable", "Disable a tunnel: it stops and is kept.", with_tunnel(&["tunnel"], false)));
        v.push(ask("ssh_tunnel_delete", "Delete a tunnel, stopping it, removing the key made for it from the vault and its trusted host key from the known hosts.", with_tunnel(&["tunnel"], false)));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => {
                let now = crate::clock::now_unix().max(0) as u64;
                let rows = self.mgr.rows();
                if rows.is_empty() {
                    return Ok("No SSH tunnels yet.".into());
                }
                Ok(rows.iter().map(|r| one_line(&r.tunnel, &r.status, now)).collect::<Vec<_>>().join("\n"))
            }
            "ssh_tunnel_add" | "ssh_tunnel_edit" => {
                let t = self.wanted(name, args)?;
                let saved = self.mgr.save(t, false)?;
                let (_, notes) = self.host_notes(&saved);
                Ok(format!("Saved “{}” ({}). Address: {}. {}\n{}", saved.name, if saved.enabled { "enabled" } else { "disabled; enable it when the host is trusted" }, saved.address(), if name == "ssh_tunnel_add" { "The public half of the key must be in the user's authorized keys on the host." } else { "" }, notes.join("\n")))
            }
            "ssh_tunnel_enable" => {
                let t = self.find(args)?;
                let (trusted, notes) = self.host_notes(&t);
                if !trusted {
                    return Err(format!("Not enabled: a host of the route is not trusted.\n{}", notes.join("\n")));
                }
                self.mgr.set_enabled(&t.id, true)?;
                self.mgr.ensure_supervised();
                self.mgr.tick(crate::clock::now_unix().max(0) as u64);
                Ok(format!("Enabled “{}”. It starts now and with the device; ssh_tunnel_list shows its state.", t.name))
            }
            "ssh_tunnel_disable" => {
                let t = self.find(args)?;
                self.mgr.set_enabled(&t.id, false)?;
                Ok(format!("Disabled “{}”; it is stopped and kept.", t.name))
            }
            "ssh_tunnel_delete" => {
                let t = self.find(args)?;
                Ok(format!("Deleted {}.", self.mgr.remove(&t.id)?.join(", ")))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if WRITE_TOOLS.contains(&name) {
            return match self.card(name, args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        if name == READ_TOOL {
            return "List the SSH tunnels".into();
        }
        self.inner.describe(name, args)
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).ok();
        }
        if name == READ_TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).err();
        }
        if name == READ_TOOL { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_tunnel_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !WRITE_TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) { Some("changing SSH tunnels asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
