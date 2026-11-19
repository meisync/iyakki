//! Routing by domain name, and the certificates of HTTPS routes (docs/development/CLOUD-MULTITENANT-PLAN.md, section 3c).
//!
//! [`RouteTable`] is the list of routes (host, path prefix, target, scheme); [`RouteTable::pick`] chooses one for a request's `Host` header
//! and path; [`ingress`] writes the Kubernetes Ingress of a table. [`check_certificate`] says whether a certificate chain and key a person
//! supplied can serve a host (the key matches, the host is covered, it is not expired, and when it runs out), with no X509 library: a
//! small reader of the few fields it needs ([`read_cert`]).
//!
//! The local listener that applies a table (sockets, SNI, WebSocket upgrade) is `cloudlisten.rs`.



#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    #[default]
    Http,
    /// TLS ends at the router with the route's certificate.
    Https,
    /// TLS is passed through untouched to the target (routed by SNI).
    Passthrough,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// A Kubernetes Service of the tenant's namespace.
    Service { name: String, port: u16 },
    /// A port on this machine.
    Local { port: u16 },
    /// An SSH tunnel of Settings -> SSH tunnels (a one-page tunnel), by its id: the listener connects to the tunnel's local port.
    Tunnel { id: String },
    /// A private address on the other side of a VPN interface (WireGuard), reached through the machine's routes.
    Peer { addr: String, port: u16 },
}

/// Where a route's certificate comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CertRef {
    /// A `kubernetes.io/tls` Secret the person made (on a cluster).
    Secret(String),
    /// A name in the vault (the local listener).
    Vault(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// Only people of the tenant (or namespace) the route belongs to, signed in.
    #[default]
    Members,
    /// Anyone. Has to be said.
    Public,
}


use serde::{Deserialize, Serialize};

use crate::cloudnames::{dns_label, dns_subdomain, host_name};





#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// `app.example.org` or `*.example.org` (one label).
    pub host: String,
    #[serde(default = "root")]
    pub path_prefix: String,
    pub target: Target,
    #[serde(default)]
    pub scheme: Scheme,
    /// The port the router listens on for this route (80 and 443 when not given).
    #[serde(default)]
    pub listen: Option<u16>,
    /// An Upgrade (WebSocket) passes through. On by default.
    #[serde(default = "yes")]
    pub websocket: bool,
    /// Plain HTTP asks are answered with a redirect to HTTPS (for `https` routes).
    #[serde(default)]
    pub redirect_http: bool,
    #[serde(default)]
    pub cert: Option<CertRef>,
    #[serde(default)]
    pub access: Access,
    /// Mutual TLS: the name (in the certificate source) of the CA that signs the client certificates this route accepts. The CA belongs to
    /// the HOST: if any https route of a host names one, every route of that host asks for a client certificate of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_ca: Option<String>,
}

fn root() -> String {
    "/".into()
}
fn yes() -> bool {
    true
}

impl Route {
    pub fn listen_port(&self) -> u16 {
        self.listen.unwrap_or(if self.scheme == Scheme::Http { 80 } else { 443 })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteTable {
    #[serde(default)]
    pub routes: Vec<Route>,
    /// The Ingress class of the cluster, when it has more than one.
    #[serde(default)]
    pub ingress_class: Option<String>,
}

pub fn wildcard_or_host(h: &str) -> bool {
    match h.strip_prefix("*.") {
        Some(rest) => host_name(rest),
        None => host_name(h),
    }
}

fn path_ok(p: &str) -> bool {
    p.starts_with('/') && p.len() <= 200 && p.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'-' | b'_' | b'.' | b'~'))
}

impl RouteTable {
    pub fn parse(text: &str) -> Result<RouteTable, String> {
        let t: RouteTable = serde_json::from_str(text).map_err(|e| format!("routes: {e}"))?;
        t.check(&[])?;
        Ok(t)
    }

    /// Refuse what could not work or could add a line of YAML. `held` are the ports other services of this machine already listen on.
    pub fn check(&self, held: &[u16]) -> Result<(), String> {
        let mut seen: Vec<(String, String, u16)> = Vec::new();
        for r in &self.routes {
            if !wildcard_or_host(&r.host) {
                return Err(format!("host \"{}\" is not a host name (app.example.org or *.example.org)", r.host));
            }
            if !path_ok(&r.path_prefix) {
                return Err(format!("path \"{}\" must start with / and use letters, digits and - _ . ~ /", r.path_prefix));
            }
            match &r.target {
                Target::Service { name, port } => {
                    if !dns_label(name) || *port == 0 {
                        return Err(format!("route {}: service \"{name}\" port {port} is not valid", r.host));
                    }
                }
                Target::Local { port } if *port == 0 => return Err(format!("route {}: port 0 is not a port", r.host)),
                Target::Local { .. } => {}
                Target::Tunnel { id } => {
                    if !crate::sshtunnel::valid_id(id) {
                        return Err(format!("route {}: \"{id}\" is not an SSH tunnel id", r.host));
                    }
                }
                Target::Peer { addr, port } => crate::cloudnames::check_peer(&r.host, addr, *port)?,
            }
            if r.listen == Some(0) {
                return Err(format!("route {}: listen port 0 is not a port", r.host));
            }
            match (&r.scheme, &r.cert) {
                (Scheme::Https, None) => return Err(format!("route {}: https needs a certificate (cert)", r.host)),
                (Scheme::Http, Some(_)) | (Scheme::Passthrough, Some(_)) => return Err(format!("route {}: a certificate belongs to an https route", r.host)),
                _ => {}
            }
            if let Some(CertRef::Secret(n)) = &r.cert {
                if !dns_subdomain(n) {
                    return Err(format!("route {}: \"{n}\" is not a Secret name", r.host));
                }
            }
            if let Some(CertRef::Vault(n)) = &r.cert {
                if !crate::cloudnames::plain_name(n) {
                    return Err(format!("route {}: \"{n}\" is not a vault name", r.host));
                }
            }
            if let Some(ca) = &r.client_ca {
                if r.scheme != Scheme::Https {
                    return Err(format!("route {}: a client CA belongs to an https route (a plain or passthrough route would ignore it)", r.host));
                }
                if !crate::cloudnames::plain_name(ca) {
                    return Err(format!("route {}: \"{ca}\" is not a name for a client CA", r.host));
                }
            }
            if r.redirect_http && r.scheme != Scheme::Https {
                return Err(format!("route {}: redirect_http is for https routes", r.host));
            }
            if r.access == Access::Public && r.scheme == Scheme::Http && r.listen_port() != 80 {
                // nothing wrong in itself; kept so a future rule has a place
            }
            if held.contains(&r.listen_port()) {
                return Err(format!("route {}: port {} is already used by another service of this machine", r.host, r.listen_port()));
            }
            let key = (r.host.to_ascii_lowercase(), r.path_prefix.clone(), r.listen_port());
            if seen.contains(&key) {
                return Err(format!("route {}{} on port {} is listed twice", r.host, r.path_prefix, key.2));
            }
            seen.push(key);
        }
        if let Some(c) = &self.ingress_class {
            if !dns_subdomain(c) {
                return Err(format!("ingress class \"{c}\" is not a name"));
            }
        }
        Ok(())
    }

    /// Ports below 1024: the listener needs the right to bind them.
    pub fn privileged_ports(&self) -> Vec<u16> {
        let mut v: Vec<u16> = self.routes.iter().map(Route::listen_port).filter(|p| *p < 1024).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The route for a request: `host` as in the Host header (a port and case are ignored), `path` its path. An exact host beats a wildcard;
    /// among routes of the winning host kind the longest path prefix wins. `listen` is the port the request came in on.
    pub fn pick(&self, host: &str, path: &str, listen: u16) -> Option<&Route> {
        let h = host.rsplit_once(':').filter(|(_, p)| p.bytes().all(|c| c.is_ascii_digit())).map_or(host, |(h, _)| h).trim_end_matches('.').to_ascii_lowercase();
        let matches_path = |r: &Route| path.starts_with(&r.path_prefix) && (r.path_prefix.ends_with('/') || path.len() == r.path_prefix.len() || path.as_bytes().get(r.path_prefix.len()) == Some(&b'/'));
        let best = |exact: bool| {
            self.routes
                .iter()
                .filter(|r| r.listen_port() == listen && matches_path(r))
                .filter(|r| {
                    let rh = r.host.to_ascii_lowercase();
                    if exact {
                        rh == h
                    } else {
                        rh.strip_prefix("*.").is_some_and(|d| h.strip_suffix(d).is_some_and(|l| l.ends_with('.') && !l[..l.len() - 1].contains('.') && l.len() > 1))
                    }
                })
                .max_by_key(|r| r.path_prefix.len())
        };
        best(true).or_else(|| best(false))
    }
}

/// The Ingress of a table in `namespace`: one rule per route, TLS per host from the route's Secret, WebSocket-friendly timeouts, a
/// redirect when asked. Routes that are not for the cluster (a `Local` target, a vault certificate, a passthrough) are left out and listed
/// in a comment so nothing is silently dropped.
pub fn ingress(namespace: &str, t: &RouteTable) -> Result<String, String> {
    t.check(&[])?;
    if !dns_label(namespace) && !dns_subdomain(namespace) {
        return Err(format!("namespace \"{namespace}\" is not a name"));
    }
    let mut rules = String::new();
    let mut tls: Vec<(String, String)> = Vec::new();
    let mut skipped = String::new();
    let mut ws = false;
    let mut redirect = false;
    for r in &t.routes {
        let Target::Service { name, port } = &r.target else {
            skipped.push_str(&format!("#   {}{}: a local port, served by the local listener\n", r.host, r.path_prefix));
            continue;
        };
        if r.scheme == Scheme::Passthrough || matches!(r.cert, Some(CertRef::Vault(_))) {
            skipped.push_str(&format!("#   {}{}: {}\n", r.host, r.path_prefix, if r.scheme == Scheme::Passthrough { "TLS passthrough needs the ingress controller's own setting" } else { "its certificate is in the vault: make a TLS Secret to serve it from the cluster" }));
            continue;
        }
        if let Some(CertRef::Secret(s)) = &r.cert {
            if !tls.iter().any(|(h, _)| *h == r.host) {
                tls.push((r.host.clone(), s.clone()));
            }
        }
        ws |= r.websocket;
        redirect |= r.redirect_http;
        rules.push_str(&format!(
            "    - host: \"{h}\"\n      http:\n        paths:\n          - path: {p}\n            pathType: Prefix\n            backend:\n              service:\n                name: {n}\n                port: {{ number: {port} }}\n",
            h = r.host,
            p = r.path_prefix,
            n = name
        ));
    }
    if rules.is_empty() {
        return Ok(format!("# no route for this cluster\n{skipped}"));
    }
    let mut y = String::new();
    if !skipped.is_empty() {
        y.push_str(&format!("# routes not written here:\n{skipped}"));
    }
    y.push_str(&format!("apiVersion: networking.k8s.io/v1\nkind: Ingress\nmetadata:\n  name: iyakki-routes\n  namespace: {namespace}\n"));
    let mut ann = String::new();
    if ws {
        ann.push_str("    nginx.ingress.kubernetes.io/proxy-read-timeout: \"3600\"\n    nginx.ingress.kubernetes.io/proxy-send-timeout: \"3600\"\n");
    }
    ann.push_str(&format!("    nginx.ingress.kubernetes.io/ssl-redirect: \"{redirect}\"\n"));
    y.push_str(&format!("  annotations:\n{ann}spec:\n"));
    if let Some(c) = &t.ingress_class {
        y.push_str(&format!("  ingressClassName: {c}\n"));
    }
    if !tls.is_empty() {
        y.push_str("  tls:\n");
        for (h, s) in &tls {
            y.push_str(&format!("    - hosts: [\"{h}\"]\n      secretName: {s}\n"));
        }
    }
    y.push_str("  rules:\n");
    y.push_str(&rules);
    y.push_str("---\n");
    Ok(y)
}

// ------------------------------------------------------------------------------------------------------------ certificates

pub use iyakki_link::certinfo::{name_covers, read_cert, CertInfo};

/// How many days ahead an expiry is warned about.
pub const WARN_DAYS: i64 = 30;

