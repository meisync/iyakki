//! The lead assistant's tools for domain names (Settings -> Domains, `docs/DOMAINS.md`): `domains_list` reads (no card);
//! `domain_route_add`, `domain_route_remove` and `domains_listener_set` each ask on a card that says in plain words what changes, ask
//! every time (also in Auto mode), and "Allow for this session" does not remember them (`agent::hard_ask`).
//!
//! * The firewall opening is its own approval: a `domains_listener_set` call that turns the listener on cannot also open the firewall, and
//!   the card of the call that opens it names the ports, the local-network scope and the end time. Closing it is part of switching off.
//! * A certificate is a NAME in the vault, or `trial` (a self-signed one is made for the host, which browsers do not trust). Key text is
//!   refused and never echoed: the person imports their own files in Settings -> Domains.
//! * The checks are the manager's own ([`crate::domains`], the route checker of `cloudroute.rs`); there is no second path around them.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::cloudroute::{Access, CertRef, Route, RouteTable, Scheme, Target};
use crate::domains::{access_text, cert_name_for, target_text, ListenerChange, Manager};

pub const PROMPT_RULE: &str = "\nDOMAIN NAMES: domains_list shows the person's domain routes (a host name such as store.local mapped to a port or cluster service on this device), the listener's state and the certificates. To add or remove a route or to switch the listener (ports, firewall opening) call domain_route_add, domain_route_remove or domains_listener_set: each asks the person every time. Switching the listener on and opening the firewall to the local network are two separate calls. A host ending in .local is also announced on the local network. Never ask for or send certificate or key text: use a vault certificate NAME or `trial`.";

pub const WRITE_TOOLS: [&str; 3] = ["domain_route_add", "domain_route_remove", "domains_listener_set"];
pub const READ_TOOL: &str = "domains_list";

pub fn is_domain_tool(n: &str) -> bool {
    n == READ_TOOL || WRITE_TOOLS.contains(&n)
}

pub struct DomainTools {
    inner: Arc<dyn Tools>,
    mgr: Arc<Manager>,
}

fn text(args: &Value, k: &str) -> String {
    args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

fn num(args: &Value, k: &str) -> Result<Option<u16>, String> {
    match args.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).and_then(|n| u16::try_from(n).ok()).filter(|n| *n > 0).map(Some).ok_or_else(|| format!("{k} is a number from 1 to 65535")),
    }
}

fn flag(args: &Value, k: &str) -> Result<Option<bool>, String> {
    match args.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(Value::String(s)) if s == "true" || s == "false" => Ok(Some(s == "true")),
        _ => Err(format!("{k} is true or false")),
    }
}

/// Text that looks like a key or certificate is refused, and never repeated.
fn reject_secrets(args: &Value) -> Result<(), String> {
    fn walk(v: &Value) -> bool {
        match v {
            Value::String(s) => s.contains("-----BEGIN") || s.contains("PRIVATE KEY") || s.contains('\n') || s.len() > 200,
            Value::Array(a) => a.iter().any(walk),
            Value::Object(o) => o.values().any(walk),
            _ => false,
        }
    }
    if walk(args) {
        return Err("a value looks like certificate or key text, or is not a short plain setting. Never send keys: use a vault certificate NAME or `trial`; the person imports their own files in Settings, Domains.".into());
    }
    Ok(())
}

impl DomainTools {
    pub fn new(inner: Arc<dyn Tools>, mgr: Arc<Manager>) -> DomainTools {
        DomainTools { inner, mgr }
    }

    /// The route a call describes, the namespace of a service target, and whether a trial certificate is to be made.
    fn wanted(&self, args: &Value) -> Result<(Route, Option<String>, bool), String> {
        reject_secrets(args)?;
        let host = text(args, "host").to_ascii_lowercase();
        if host.is_empty() {
            return Err("host is the name to serve, for example store.local or app.example.org".into());
        }
        let path = if text(args, "path").is_empty() { "/".to_string() } else { text(args, "path") };
        let port = num(args, "port")?;
        let service = text(args, "service");
        let (target, ns) = match (port, service.is_empty()) {
            (Some(p), true) => (Target::Local { port: p }, None),
            (None, false) => {
                let (ns, name) = service.split_once('/').ok_or("service is NAMESPACE/NAME (domains_list shows what the cluster has)")?;
                let sp = num(args, "service_port")?.ok_or("service_port is the Service's port")?;
                (Target::Service { name: name.to_string(), port: sp }, Some(ns.to_string()))
            }
            _ => return Err("give either port (a port on this device, such as a NodePort) or service and service_port (a cluster Service), not both".into()),
        };
        let scheme = match text(args, "scheme").as_str() {
            "" | "http" => Scheme::Http,
            "https" => Scheme::Https,
            "passthrough" => Scheme::Passthrough,
            _ => return Err("scheme is http, https or passthrough".into()),
        };
        let access = match text(args, "access").as_str() {
            "" | "members" => Access::Members,
            "public" => Access::Public,
            _ => return Err("access is members or public".into()),
        };
        let cert = text(args, "certificate");
        let trial = cert == "trial";
        let cert_ref = match (scheme, cert.is_empty()) {
            (Scheme::Https, true) => return Err("an https route needs a certificate: a vault certificate NAME (domains_list shows them) or `trial`".into()),
            (Scheme::Https, false) => Some(CertRef::Vault(if trial { cert_name_for(&host) } else { cert })),
            (_, false) => return Err("a certificate belongs to an https route".into()),
            _ => None,
        };
        let route = Route { host, path_prefix: path, target, scheme, listen: None, websocket: flag(args, "websocket")?.unwrap_or(true), redirect_http: false, cert: cert_ref, access, client_ca: None };
        RouteTable { routes: vec![route.clone()], ingress_class: None }.check(&[])?;
        if let (Some(CertRef::Vault(n)), false) = (&route.cert, trial) {
            if !self.mgr.certs().map(|c| c.iter().any(|r| r.name == *n)).unwrap_or(false) {
                return Err(format!("there is no certificate called \"{n}\" in the vault; use `trial` for a self-signed one, or ask the person to import theirs in Settings, Domains"));
            }
        }
        Ok((route, ns, trial))
    }

    fn change(&self, args: &Value) -> Result<ListenerChange, String> {
        reject_secrets(args)?;
        let c = ListenerChange { enabled: flag(args, "enabled")?, http_port: num(args, "http_port")?, https_port: num(args, "https_port")?, standard_ports_approved: flag(args, "standard_ports")?.unwrap_or(false), firewall: flag(args, "open_firewall")?, firewall_hours: num(args, "expires_hours")?.map(u32::from) };
        if c.enabled.is_none() && c.http_port.is_none() && c.https_port.is_none() && c.firewall.is_none() {
            return Err("nothing to change: give enabled, http_port, https_port or open_firewall".into());
        }
        let was = self.mgr.settings();
        if c.enabled == Some(true) && c.firewall == Some(true) && !was.enabled {
            return Err("turning the listener on and opening the firewall are two steps with an approval each: call once with enabled=true, then again with open_firewall=true".into());
        }
        if c.firewall == Some(true) && !was.enabled && c.enabled != Some(true) {
            return Err("the listener is off; switch it on first (enabled=true), then open the firewall in a second call".into());
        }
        Ok(c)
    }

    fn card(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "domain_route_add" => {
                let (r, ns, trial) = self.wanted(args)?;
                Ok(format!(
                    "Add the domain route {}{}.\n• Goes to: {}{}\n• Served as: {}{}\n• WebSocket passes through: {}\n• Who may reach it: {}\n• This does not open the firewall; the listener's state and firewall are separate steps.{}",
                    r.host,
                    r.path_prefix,
                    target_text(&r.target),
                    ns.map(|n| format!(" (namespace {n})")).unwrap_or_default(),
                    match r.scheme { Scheme::Http => "plain http", Scheme::Https => "https", Scheme::Passthrough => "TLS passed through untouched" },
                    if trial { " with a new self-signed trial certificate kept in your vault (browsers will warn)" } else { "" },
                    if r.websocket { "yes" } else { "no" },
                    access_text(r.access),
                    if r.host.ends_with(".local") { "\n• The name ends in .local: while the listener runs it is announced on the local network as another name for this device." } else { "" }
                ))
            }
            "domain_route_remove" => {
                let r = self.find(args)?;
                Ok(format!("Remove the domain route {}{} (it went to {}). Nothing else changes; certificates stay in the vault.", r.host, r.path_prefix, target_text(&r.target)))
            }
            "domains_listener_set" => {
                let c = self.change(args)?;
                let was = self.mgr.settings();
                let mut lines = Vec::new();
                match c.enabled {
                    Some(true) => lines.push("• Turn the domain listener ON. It listens on this device only (127.0.0.1) until the firewall is opened in a separate step.".to_string()),
                    Some(false) => lines.push("• Turn the domain listener OFF: the ports close and any firewall opening is removed.".into()),
                    None => {}
                }
                if let Some(p) = c.http_port {
                    lines.push(format!("• http port {} (now {}){}", p, was.http_port, if p < 1024 { ": a standard web port, below 1024, needs the right to bind it" } else { "" }));
                }
                if let Some(p) = c.https_port {
                    lines.push(format!("• https port {} (now {}){}", p, was.https_port, if p < 1024 { ": a standard web port, below 1024, needs the right to bind it" } else { "" }));
                }
                match c.firewall {
                    Some(true) => lines.push(format!(
                        "• FIREWALL: open TCP ports {} and {} to the LOCAL NETWORK only (private and link-local addresses, never the internet), {}. Any computer on the local network can then reach every route marked as served here. Closing it again is done by turning the listener off.",
                        c.http_port.unwrap_or(was.http_port),
                        c.https_port.unwrap_or(was.https_port),
                        match c.firewall_hours { Some(h) if h > 0 => format!("for {h} hour(s), after which the listener is switched off"), _ => "with no end time".to_string() }
                    )),
                    Some(false) => lines.push("• FIREWALL: close the opening again (the listener stays on, on this device only).".into()),
                    None => {}
                }
                Ok(format!("Change the domain listener:\n{}", lines.join("\n")))
            }
            _ => Err("not a domains tool".into()),
        }
    }

    fn find(&self, args: &Value) -> Result<Route, String> {
        let host = text(args, "host");
        let path = if text(args, "path").is_empty() { "/".to_string() } else { text(args, "path") };
        if host.is_empty() {
            return Err("name the route's host (domains_list shows them)".into());
        }
        self.mgr.route(&host, &path).ok_or_else(|| format!("there is no route for {host}{path} (domains_list shows them)"))
    }

    fn list(&self) -> String {
        let st = self.mgr.status();
        let mut out = vec![format!("Listener: {}", st.line())];
        let cfg = self.mgr.settings();
        out.push(format!("Ports: http {}, https {}. Firewall opening: {}.", cfg.http_port, cfg.https_port, if cfg.firewall { "open to the local network" } else { "closed" }));
        if !st.names.is_empty() {
            out.push(format!("Announced on the local network (needs \"Announce this device\" on): {}", st.names.join(", ")));
        }
        let routes = self.mgr.routes();
        if routes.is_empty() {
            out.push("No domain routes yet.".into());
        }
        for r in &routes {
            out.push(format!(
                "{}{} -> {} · {} · websocket {} · {}{}",
                r.host,
                r.path_prefix,
                target_text(&r.target),
                match r.scheme { Scheme::Http => "http", Scheme::Https => "https", Scheme::Passthrough => "passthrough" },
                if r.websocket { "on" } else { "off" },
                access_text(r.access),
                self.mgr.cert_state(r).map(|c| format!(" · certificate: {c}")).unwrap_or_default()
            ));
        }
        match self.mgr.certs() {
            Ok(c) if !c.is_empty() => out.extend(c.iter().map(|r| format!("certificate {}", r.line()))),
            Ok(_) => {}
            Err(e) => out.push(format!("Certificates could not be listed: {e}")),
        }
        match self.mgr.discover() {
            Ok(f) => {
                for s in f.iter().filter(|s| s.namespace != "kube-system") {
                    for p in &s.ports {
                        out.push(format!("found in the cluster: {}", s.label(p)));
                    }
                }
            }
            Err(e) => out.push(format!("Cluster services not listed: {e}")),
        }
        out.extend(st.last_errors.iter().map(|e| format!("recent problem: {e}")));
        out.join("\n")
    }
}

impl Tools for DomainTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_domain_tool(&d.spec.name));
        let ask = |name: &str, desc: &str, schema: Value| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk: Risk::Ask, untrusted: false };
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "List the person's domain routes (host name to port or cluster service), the listener's state, the certificates in the vault and the services found in the local cluster. No secrets.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ask(
            "domain_route_add",
            "Add a domain route: a host name (for example store.local) served by this device's listener and sent to a port on this device (a NodePort) or a cluster Service. A host ending in .local is announced on the local network. The person approves a card. It does not start the listener or open the firewall.",
            json!({"type": "object", "properties": {
                "host": {"type": "string", "description": "store.local, app.example.org or *.example.org"},
                "path": {"type": "string", "description": "Path prefix, / by default."},
                "port": {"type": "integer", "description": "A port on this device (for example the private store's NodePort 30580). Use this or service."},
                "service": {"type": "string", "description": "A cluster Service as NAMESPACE/NAME (domains_list shows them). With service_port."},
                "service_port": {"type": "integer"},
                "scheme": {"type": "string", "enum": ["http", "https", "passthrough"], "description": "http (default), https (needs certificate) or passthrough (TLS passed through to the target)."},
                "certificate": {"type": "string", "description": "https only: a certificate NAME from the vault, or `trial` for a self-signed one. Never certificate text."},
                "websocket": {"type": "boolean", "description": "Let WebSocket upgrades through (default true)."},
                "access": {"type": "string", "enum": ["members", "public"], "description": "members (default; not enforced by the listener) or public."}
            }, "required": ["host"]}),
        ));
        v.push(ask("domain_route_remove", "Remove a domain route. The person approves a card.", json!({"type": "object", "properties": {"host": {"type": "string"}, "path": {"type": "string", "description": "/ by default."}}, "required": ["host"]})));
        v.push(ask(
            "domains_listener_set",
            "Turn the domain listener on or off, change its ports, or open the firewall to the LOCAL NETWORK for it. The person approves a card each time. Turning it on and opening the firewall are two separate calls. It starts on 127.0.0.1 with ports 8080 and 8443; ports 80 and 443 need standard_ports=true (and approval).",
            json!({"type": "object", "properties": {
                "enabled": {"type": "boolean"},
                "http_port": {"type": "integer"},
                "https_port": {"type": "integer"},
                "standard_ports": {"type": "boolean", "description": "Set when http_port 80 or https_port 443 is meant; the card shows it."},
                "open_firewall": {"type": "boolean", "description": "true: open the listener's ports to the local network (only when the listener is already on). false: close the opening."},
                "expires_hours": {"type": "integer", "description": "With open_firewall=true: hours until the opening ends and the listener is switched off. Leave out for no end time."}
            }}),
        ));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => Ok(self.list()),
            "domain_route_add" => {
                let (mut r, ns, trial) = self.wanted(args)?;
                if trial {
                    let n = self.mgr.make_trial_cert(&r.host)?;
                    r.cert = Some(CertRef::Vault(n));
                }
                self.mgr.save_route(None, r.clone(), ns.as_deref())?;
                let st = self.mgr.status();
                Ok(format!(
                    "Added {}{} -> {}. {}{}",
                    r.host,
                    r.path_prefix,
                    target_text(&r.target),
                    if st.running { "The listener is running and serves it now." } else { "The listener is off: switch it on with domains_listener_set before the name works." },
                    if st.running && !self.mgr.settings().firewall { " It listens on this device only; other computers need the firewall step." } else { "" }
                ))
            }
            "domain_route_remove" => {
                let r = self.find(args)?;
                self.mgr.remove_route(&r.host, &r.path_prefix)?;
                Ok(format!("Removed {}{}.", r.host, r.path_prefix))
            }
            "domains_listener_set" => {
                let c = self.change(args)?;
                self.mgr.set_listener(c)
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
            return "List the domain routes".into();
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
        !is_domain_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !WRITE_TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) { Some("changing domain names and the listener asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

#[cfg(test)]
#[path = "domainstools_tests.rs"]
mod tests;
