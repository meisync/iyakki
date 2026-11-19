//! Types moved out of `iyakki_agent::domains` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// What a change of the listener asks for. Fields left `None` stay as they are.
#[derive(Clone, Debug, Default)]
pub struct ListenerChange {
    pub enabled: Option<bool>,
    pub http_port: Option<u16>,
    pub https_port: Option<u16>,
    /// The caller has the person's approval for ports 80 and 443 (below 1024 the listener needs the right to bind them).
    pub standard_ports_approved: bool,
    /// Open (true) or close (false) the ports to the local network in the firewall.
    pub firewall: Option<bool>,
    /// With `firewall`: hours until the opening ends (None or 0: no end time).
    pub firewall_hours: Option<u32>,
}

use crate::cloudroute::Scheme;
use crate::cloudroute::Target;

/// One TCP port of a cluster Service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundPort {
    pub name: String,
    pub port: u16,
    pub node_port: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub namespace: String,
    pub name: String,
    /// `ClusterIP`, `NodePort` or `LoadBalancer`.
    pub kind: String,
    pub ports: Vec<FoundPort>,
}

impl Found {
    /// The target a route to this port gets: a NodePort is reached on this machine at 127.0.0.1 (kube-proxy answers it there), anything
    /// else through the Service (a `kubectl port-forward` the listener runs).
    pub fn target(&self, p: &FoundPort) -> Target {
        match p.node_port {
            Some(n) => Target::Local { port: n },
            None => Target::Service { name: self.name.clone(), port: p.port },
        }
    }

    pub fn label(&self, p: &FoundPort) -> String {
        match p.node_port {
            Some(n) => format!("{}/{} port {} (NodePort {n}, reached on this device)", self.namespace, self.name, p.port),
            None => format!("{}/{} port {} (inside the cluster, reached through kubectl)", self.namespace, self.name, p.port),
        }
    }
}

/// A service of this device itself that a route can point at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Own {
    pub label: String,
    pub target: Target,
    pub scheme: Scheme,
    pub note: String,
}

// ---- moved from iyakki_agent::domains (F-SPLIT-03) ----

use serde::{Deserialize, Serialize};
use crate::cloudroute::{Access, WARN_DAYS};

pub const DEFAULT_HTTP: u16 = 8080;
pub const DEFAULT_HTTPS: u16 = 8443;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListenerCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "d_http")]
    pub http_port: u16,
    #[serde(default = "d_https")]
    pub https_port: u16,
    /// The ports are open to the local network in the firewall (and the listener is bound to all addresses). Off: 127.0.0.1 only.
    #[serde(default)]
    pub firewall: bool,
    /// Unix seconds when the opening ends (0: no end time). Only with `firewall`.
    #[serde(default)]
    pub firewall_until: i64,
}

fn d_http() -> u16 {
    DEFAULT_HTTP
}
fn d_https() -> u16 {
    DEFAULT_HTTPS
}

impl Default for ListenerCfg {
    fn default() -> ListenerCfg {
        ListenerCfg { enabled: false, http_port: DEFAULT_HTTP, https_port: DEFAULT_HTTPS, firewall: false, firewall_until: 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertRow {
    pub name: String,
    pub names: Vec<String>,
    /// Days until it runs out (negative: ran out).
    pub days: i64,
    /// Routes that use it.
    pub used_by: Vec<String>,
    pub problem: Option<String>,
}

impl CertRow {
    pub fn line(&self) -> String {
        let when = if self.days < 0 { "expired".to_string() } else if self.days <= WARN_DAYS { format!("runs out in {} day(s)", self.days) } else { format!("valid for {} more days", self.days) };
        match &self.problem {
            Some(p) => format!("{}: {p}", self.name),
            None => format!("{} covers {}: {when}{}", self.name, self.names.join(", "), if self.used_by.is_empty() { "" } else { " (in use)" }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub enabled: bool,
    pub running: bool,
    pub bind: String,
    pub ports: Vec<u16>,
    pub firewall: bool,
    pub firewall_until: i64,
    /// Ports the firewall gate has open now.
    pub firewall_ports: Vec<u16>,
    pub names: Vec<String>,
    pub last_errors: Vec<String>,
    /// Hosts that are not served over https, and why.
    pub refused: Vec<(String, String)>,
}

impl Status {
    pub fn line(&self) -> String {
        if !self.enabled {
            return "Off: nothing listens for domain names".into();
        }
        if !self.running {
            return format!("On, but not listening{}", self.last_errors.last().map(|e| format!(": {e}")).unwrap_or_default());
        }
        let ports = self.ports.iter().map(u16::to_string).collect::<Vec<_>>().join(", ");
        if self.firewall {
            format!("Listening on port {ports}, open to the local network{}", if self.firewall_until > 0 { " until the end time" } else { "" })
        } else {
            format!("Listening on port {ports}, on this device only ({}); other computers cannot reach it until you open the firewall", self.bind)
        }
    }
}

pub fn target_text(t: &Target) -> String {
    match t {
        Target::Local { port } => format!("port {port} on this device"),
        Target::Service { name, port } => format!("service {name}:{port}"),
        Target::Tunnel { id } => format!("SSH tunnel {id}"),
        Target::Peer { addr, port } => format!("VPN peer {addr}:{port}"),
    }
}

pub fn access_text(a: Access) -> &'static str {
    match a {
        Access::Members => "people of this device who are signed in (the listener checks the session)",
        Access::Public => "anyone who can reach the port",
    }
}

/// A vault name for the certificate of a host: `store.local` is `store-local`, `*.x.local` is `wild-x-local`.
pub fn cert_name_for(host: &str) -> String {
    let mut s: String = host.to_ascii_lowercase().replace("*.", "wild-").chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    s.truncate(40);
    s.trim_matches('-').to_string()
}
