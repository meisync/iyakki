//! The firewall's settings: what the person chose. Everything is plain text and numbers (so it can be saved, exported and
//! imported as JSON); [`crate::validate`] turns a [`Rule`] into typed values and [`crate::ruleset`] turns the lot into rules.

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// What a rule matches on the other end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Host,
    Ip,
    Range,
    Cidr,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Host => "Hostname",
            Kind::Ip => "IP",
            Kind::Range => "IP range",
            Kind::Cidr => "CIDR",
        }
    }
    pub const ALL: [Kind; 4] = [Kind::Host, Kind::Ip, Kind::Range, Kind::Cidr];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Proto {
    Tcp,
    Udp,
    Both,
}

impl Proto {
    pub fn label(self) -> &'static str {
        match self {
            Proto::Tcp => "TCP",
            Proto::Udp => "UDP",
            Proto::Both => "TCP+UDP",
        }
    }
}

/// One allow rule. Rules only allow (the default is to block), so their order never matters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: u32,
    pub enabled: bool,
    pub kind: Kind,
    /// An address, `a-b`, `a/len` or a host name, according to `kind`.
    pub value: String,
    pub proto: Proto,
    /// `22`, `80,443`, `8000-8100`; empty means every port.
    pub ports: String,
    /// Only packets that arrive on this interface; empty means any.
    pub iface: String,
    pub comment: String,
    /// Unix seconds after which the rule no longer applies; 0 means never.
    pub expires: i64,
}

impl Rule {
    pub fn new(kind: Kind, value: &str) -> Rule {
        Rule { id: 0, enabled: true, kind, value: value.trim().to_string(), proto: Proto::Both, ports: String::new(), iface: String::new(), comment: String::new(), expires: 0 }
    }
}

/// One-click groups of rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    Ssh,
    RemoteDesktop,
    SharedFolders,
    K3sApi,
    NodePorts,
    StoreNodePort,
    DnsService,
    WebServer,
}

impl Preset {
    pub const ALL: [Preset; 8] = [Preset::Ssh, Preset::RemoteDesktop, Preset::SharedFolders, Preset::K3sApi, Preset::NodePorts, Preset::StoreNodePort, Preset::DnsService, Preset::WebServer];
    pub fn id(self) -> &'static str {
        match self {
            Preset::Ssh => "ssh",
            Preset::RemoteDesktop => "remote",
            Preset::SharedFolders => "shares",
            Preset::K3sApi => "k3s-api",
            Preset::NodePorts => "nodeports",
            Preset::StoreNodePort => "store",
            Preset::DnsService => "dns",
            Preset::WebServer => "web",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Preset::Ssh => "SSH (port 22)",
            Preset::RemoteDesktop => "Remote desktop / web mode",
            Preset::SharedFolders => "Shared folders",
            Preset::K3sApi => "Kubernetes API (6443)",
            Preset::NodePorts => "Kubernetes NodePorts (30000-32767)",
            Preset::StoreNodePort => "Private store (NodePort 30300)",
            Preset::DnsService => "Allow DNS service (port 53) to this machine",
            Preset::WebServer => "Web server (ports 80 and 443)",
        }
    }
    pub fn explain(self) -> &'static str {
        match self {
            Preset::Ssh => "Lets other computers on your local network open an SSH session, when an SSH server runs on this machine.",
            Preset::RemoteDesktop => "Lets a browser on your local network (or anywhere, on a cloud machine) open this machine's web desktop. Without it you cannot reach web mode from another device.",
            Preset::SharedFolders => "Lets your local network reach shared folders (ports 8081-8180, where Shared folders picks its ports).",
            Preset::K3sApi => "Lets other computers on your local network talk to this machine's Kubernetes API. Off keeps the cluster private to this machine.",
            Preset::NodePorts => "Lets your local network reach services the dev cluster publishes on NodePorts. Local use on this machine always works without it.",
            Preset::StoreNodePort => "Lets your local network reach the private app store's NodePort. This machine itself always can.",
            Preset::DnsService => "For a machine that runs a DNS server: lets your local network ask it (TCP and UDP 53). Looking names up from this machine always works without it.",
            Preset::WebServer => "For a machine that runs a web server: lets your local network open its web pages (TCP 80 and 443).",
        }
    }
    pub fn from_id(s: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.id() == s)
    }
}

/// Answering ping (ICMP echo).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ping {
    Off,
    Local,
    Anyone,
}

impl Ping {
    pub fn label(self) -> &'static str {
        match self {
            Ping::Off => "ping is not answered",
            Ping::Local => "ping is answered from the local network only",
            Ping::Anyone => "ping is answered from anywhere",
        }
    }
    pub fn next(self) -> Ping {
        match self {
            Ping::Off => Ping::Local,
            Ping::Local => Ping::Anyone,
            Ping::Anyone => Ping::Off,
        }
    }
}

/// The saved settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub enabled: bool,
    pub rules: Vec<Rule>,
    pub presets: Vec<Preset>,
    pub ping: Ping,
    /// Packets from the cluster and container networks on this machine (cni0, flannel, veth, docker0, podman) are not blocked.
    pub trust_containers: bool,
    /// Minutes between looking up the host names again.
    pub refresh_minutes: u32,
    pub next_id: u32,
}

pub const CONFIG_VERSION: u32 = 1;
pub const MAX_RULES: usize = 200;

impl Default for Config {
    fn default() -> Config {
        Config { version: CONFIG_VERSION, enabled: true, rules: Vec::new(), presets: vec![Preset::RemoteDesktop], ping: Ping::Local, trust_containers: true, refresh_minutes: 5, next_id: 1 }
    }
}

impl Config {
    pub fn has(&self, p: Preset) -> bool {
        self.presets.contains(&p)
    }
    pub fn set_preset(&mut self, p: Preset, on: bool) {
        self.presets.retain(|x| *x != p);
        if on {
            self.presets.push(p);
            self.presets.sort();
        }
    }
    /// Add a rule (its id is made here). The caller validates with [`crate::validate::check_rule`] first.
    pub fn add(&mut self, mut r: Rule) -> u32 {
        r.id = self.next_id;
        self.next_id += 1;
        let id = r.id;
        self.rules.push(r);
        id
    }
    pub fn remove(&mut self, id: u32) -> bool {
        let n = self.rules.len();
        self.rules.retain(|r| r.id != id);
        self.rules.len() != n
    }
    pub fn rule_mut(&mut self, id: u32) -> Option<&mut Rule> {
        self.rules.iter_mut().find(|r| r.id == id)
    }
    /// Every distinct host name in an enabled rule, in rule order.
    pub fn host_names(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for r in self.rules.iter().filter(|r| r.enabled && r.kind == Kind::Host) {
            let n = crate::validate::normalize_host(&r.value);
            if !v.contains(&n) {
                v.push(n);
            }
        }
        v
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
    /// Read exported rules. Unknown versions and invalid rules are refused with the reason; nothing is half-imported.
    pub fn from_json(text: &str) -> Result<Config, String> {
        let mut c: Config = serde_json::from_str(text).map_err(|e| format!("This is not a firewall export: {e}"))?;
        if c.version != CONFIG_VERSION {
            return Err(format!("This export is version {}; this firewall reads version {CONFIG_VERSION}.", c.version));
        }
        if c.rules.len() > MAX_RULES {
            return Err(format!("At most {MAX_RULES} rules."));
        }
        let mut seen = std::collections::BTreeSet::new();
        for r in &c.rules {
            if !seen.insert(r.id) {
                return Err(format!("Rule number {} appears twice.", r.id));
            }
            crate::validate::check_rule(r).map_err(|e| format!("Rule {} ({}): {e}", r.id, r.value))?;
        }
        c.next_id = c.rules.iter().map(|r| r.id).max().unwrap_or(0).max(c.next_id.saturating_sub(1)) + 1;
        c.refresh_minutes = c.refresh_minutes.clamp(1, 1440);
        c.presets.sort();
        c.presets.dedup();
        Ok(c)
    }
}

/// The addresses a host name had when it was last looked up successfully.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    pub addrs: BTreeMap<String, Vec<IpAddr>>,
    /// Unix seconds of the last successful lookup per name.
    pub at: BTreeMap<String, i64>,
}
