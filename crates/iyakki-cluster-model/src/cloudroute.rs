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
                    if !crate::cloudnames::valid_tunnel_id(id) {
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


/// How many days ahead an expiry is warned about.
pub const WARN_DAYS: i64 = 30;

#[cfg(test)]
mod tests {
    use super::*;

    fn route(host: &str, path: &str, target: Target, scheme: Scheme) -> Route {
        Route { host: host.into(), path_prefix: path.into(), target, scheme, listen: None, websocket: true, redirect_http: false, cert: None, access: Access::Members, client_ca: None }
    }

    fn table(routes: Vec<Route>) -> RouteTable {
        RouteTable { routes, ingress_class: None }
    }

    fn local(port: u16) -> Target {
        Target::Local { port }
    }

    fn svc(name: &str, port: u16) -> Target {
        Target::Service { name: name.into(), port }
    }

    #[test]
    fn a_plain_or_passthrough_route_listens_on_80_or_443_unless_a_port_is_given() {
        assert_eq!(route("a.example.org", "/", local(1), Scheme::Http).listen_port(), 80);
        assert_eq!(route("a.example.org", "/", local(1), Scheme::Https).listen_port(), 443);
        assert_eq!(route("a.example.org", "/", local(1), Scheme::Passthrough).listen_port(), 443);
        let mut r = route("a.example.org", "/", local(1), Scheme::Https);
        r.listen = Some(8443);
        assert_eq!(r.listen_port(), 8443);
    }

    #[test]
    fn a_wildcard_or_a_plain_host_name_is_accepted_and_nothing_else() {
        assert!(wildcard_or_host("*.example.org"));
        assert!(wildcard_or_host("app.example.org"));
        assert!(!wildcard_or_host("example"));
        assert!(!wildcard_or_host("bad host.org"));
    }

    #[test]
    fn a_plain_route_to_a_local_port_passes_the_check() {
        assert_eq!(table(vec![route("app.example.org", "/", local(8080), Scheme::Http)]).check(&[]), Ok(()));
    }

    #[test]
    fn check_refuses_a_host_that_is_not_a_host_name() {
        let err = table(vec![route("not a host", "/", local(8080), Scheme::Http)]).check(&[]).unwrap_err();
        assert!(err.contains("is not a host name"));
    }

    #[test]
    fn check_refuses_a_path_that_does_not_start_with_a_slash() {
        let err = table(vec![route("app.example.org", "app", local(8080), Scheme::Http)]).check(&[]).unwrap_err();
        assert!(err.contains("must start with /"));
    }

    #[test]
    fn check_refuses_port_zero_for_a_local_target() {
        let err = table(vec![route("app.example.org", "/", local(0), Scheme::Http)]).check(&[]).unwrap_err();
        assert!(err.contains("port 0 is not a port"));
    }

    #[test]
    fn check_refuses_a_service_with_a_name_that_is_not_a_dns_label_or_a_port_of_zero() {
        let bad_name = table(vec![route("app.example.org", "/", svc("Web_App", 80), Scheme::Http)]).check(&[]).unwrap_err();
        assert!(bad_name.contains("is not valid"));
        assert!(table(vec![route("app.example.org", "/", svc("web", 0), Scheme::Http)]).check(&[]).is_err());
    }

    #[test]
    fn check_refuses_an_https_route_without_a_certificate() {
        let err = table(vec![route("app.example.org", "/", local(8080), Scheme::Https)]).check(&[]).unwrap_err();
        assert!(err.contains("https needs a certificate"));
    }

    #[test]
    fn check_refuses_a_certificate_on_a_plain_route() {
        let mut r = route("app.example.org", "/", local(8080), Scheme::Http);
        r.cert = Some(CertRef::Secret("app-tls".into()));
        assert!(table(vec![r]).check(&[]).unwrap_err().contains("a certificate belongs to an https route"));
    }

    #[test]
    fn check_refuses_a_certificate_secret_or_vault_name_that_is_not_valid() {
        let mut secret = route("app.example.org", "/", local(8080), Scheme::Https);
        secret.cert = Some(CertRef::Secret("Bad_Secret".into()));
        assert!(table(vec![secret]).check(&[]).unwrap_err().contains("is not a Secret name"));
        let mut vault = route("app.example.org", "/", local(8080), Scheme::Https);
        vault.cert = Some(CertRef::Vault("bad name".into()));
        assert!(table(vec![vault]).check(&[]).unwrap_err().contains("is not a vault name"));
    }

    #[test]
    fn check_refuses_a_client_ca_on_a_route_that_is_not_https_or_with_a_bad_name() {
        let mut plain = route("app.example.org", "/", local(8080), Scheme::Http);
        plain.client_ca = Some("ca-main".into());
        assert!(table(vec![plain]).check(&[]).unwrap_err().contains("a client CA belongs to an https route"));
        let mut bad = route("app.example.org", "/", local(8080), Scheme::Https);
        bad.cert = Some(CertRef::Secret("app-tls".into()));
        bad.client_ca = Some("bad ca".into());
        assert!(table(vec![bad]).check(&[]).unwrap_err().contains("is not a name for a client CA"));
    }

    #[test]
    fn check_refuses_redirect_http_on_a_route_that_is_not_https() {
        let mut r = route("app.example.org", "/", local(8080), Scheme::Http);
        r.redirect_http = true;
        assert!(table(vec![r]).check(&[]).unwrap_err().contains("redirect_http is for https routes"));
    }

    #[test]
    fn check_refuses_a_listen_port_of_zero() {
        let mut r = route("app.example.org", "/", local(8080), Scheme::Http);
        r.listen = Some(0);
        assert!(table(vec![r]).check(&[]).unwrap_err().contains("listen port 0 is not a port"));
    }

    #[test]
    fn check_refuses_a_port_another_service_of_this_machine_already_holds() {
        let err = table(vec![route("app.example.org", "/", local(8080), Scheme::Http)]).check(&[80]).unwrap_err();
        assert!(err.contains("already used by another service of this machine"));
    }

    #[test]
    fn check_refuses_the_same_host_path_and_port_listed_twice() {
        let twice = table(vec![route("app.example.org", "/", local(8080), Scheme::Http), route("app.example.org", "/", local(8081), Scheme::Http)]);
        assert!(twice.check(&[]).unwrap_err().contains("is listed twice"));
    }

    #[test]
    fn check_refuses_an_upper_case_host_name_because_host_names_must_be_lower_case() {
        let upper = table(vec![route("APP.example.org", "/", local(8080), Scheme::Http)]);
        assert!(upper.check(&[]).unwrap_err().contains("is not a host name"));
    }

    #[test]
    fn check_accepts_the_same_host_with_different_path_prefixes() {
        let two = table(vec![route("app.example.org", "/", local(8080), Scheme::Http), route("app.example.org", "/api", local(8081), Scheme::Http)]);
        assert_eq!(two.check(&[]), Ok(()));
    }

    #[test]
    fn check_refuses_a_peer_address_that_is_not_private_and_accepts_a_private_one() {
        let public = table(vec![route("peer.example.org", "/", Target::Peer { addr: "8.8.8.8".into(), port: 443 }, Scheme::Http)]);
        assert!(public.check(&[]).unwrap_err().contains("is not a private address"));
        let private = table(vec![route("peer.example.org", "/", Target::Peer { addr: "10.8.0.2".into(), port: 443 }, Scheme::Http)]);
        assert_eq!(private.check(&[]), Ok(()));
    }

    #[test]
    fn check_refuses_a_tunnel_id_that_is_not_t_followed_by_digits() {
        assert!(table(vec![route("app.example.org", "/", Target::Tunnel { id: "x9".into() }, Scheme::Http)]).check(&[]).unwrap_err().contains("is not an SSH tunnel id"));
        assert_eq!(table(vec![route("app.example.org", "/", Target::Tunnel { id: "t12".into() }, Scheme::Http)]).check(&[]), Ok(()));
    }

    #[test]
    fn check_refuses_an_ingress_class_that_is_not_a_name() {
        let mut t = table(vec![]);
        t.ingress_class = Some("Bad Class".into());
        assert!(t.check(&[]).unwrap_err().contains("is not a name"));
    }

    #[test]
    fn parse_reads_a_table_and_fills_the_defaults_for_missing_route_fields() {
        let t = RouteTable::parse(r#"{"routes":[{"host":"app.example.org","target":{"local":{"port":8080}}}]}"#).unwrap();
        let r = &t.routes[0];
        assert_eq!(r.path_prefix, "/");
        assert_eq!(r.scheme, Scheme::Http);
        assert!(r.websocket);
        assert_eq!(r.access, Access::Members);
        assert_eq!(r.listen, None);
        assert_eq!(t.ingress_class, None);
    }

    #[test]
    fn parse_accepts_an_empty_object_as_an_empty_table() {
        assert_eq!(RouteTable::parse("{}"), Ok(RouteTable::default()));
    }

    #[test]
    fn parse_refuses_unknown_fields_and_names_the_routes_in_the_error() {
        assert!(RouteTable::parse(r#"{"routes":[],"extra":1}"#).unwrap_err().starts_with("routes: "));
    }

    #[test]
    fn parse_runs_the_same_checks_as_check() {
        let text = r#"{"routes":[{"host":"not a host","target":{"local":{"port":8080}}}]}"#;
        assert!(RouteTable::parse(text).unwrap_err().contains("is not a host name"));
    }

    #[test]
    fn pick_prefers_an_exact_host_over_a_wildcard_for_the_same_request() {
        let t = table(vec![route("*.example.org", "/", local(9000), Scheme::Http), route("app.example.org", "/", local(9001), Scheme::Http)]);
        assert_eq!(t.pick("app.example.org", "/", 80).map(|r| r.target.clone()), Some(local(9001)));
    }

    #[test]
    fn pick_uses_a_wildcard_only_for_exactly_one_label_below_its_domain() {
        let t = table(vec![route("*.example.org", "/", local(9000), Scheme::Http)]);
        assert_eq!(t.pick("shop.example.org", "/", 80).map(|r| r.target.clone()), Some(local(9000)));
        assert!(t.pick("a.b.example.org", "/", 80).is_none());
        assert!(t.pick("example.org", "/", 80).is_none());
    }

    #[test]
    fn pick_ignores_the_port_in_the_host_header_its_case_and_a_trailing_dot() {
        let t = table(vec![route("app.example.org", "/", local(9001), Scheme::Http)]);
        assert_eq!(t.pick("APP.example.org:8080", "/", 80).map(|r| r.target.clone()), Some(local(9001)));
        assert_eq!(t.pick("app.example.org.", "/", 80).map(|r| r.target.clone()), Some(local(9001)));
    }

    #[test]
    fn pick_takes_the_longest_path_prefix_that_matches_on_a_segment_boundary() {
        let t = table(vec![route("app.example.org", "/", local(1), Scheme::Http), route("app.example.org", "/api", local(2), Scheme::Http)]);
        assert_eq!(t.pick("app.example.org", "/api/v1", 80).map(|r| r.target.clone()), Some(local(2)));
        assert_eq!(t.pick("app.example.org", "/api", 80).map(|r| r.target.clone()), Some(local(2)));
        assert_eq!(t.pick("app.example.org", "/apple", 80).map(|r| r.target.clone()), Some(local(1)));
        assert_eq!(t.pick("app.example.org", "/", 80).map(|r| r.target.clone()), Some(local(1)));
    }

    #[test]
    fn pick_only_matches_routes_on_the_port_the_request_came_in_on() {
        let t = table(vec![route("app.example.org", "/", local(1), Scheme::Http)]);
        assert!(t.pick("app.example.org", "/", 8080).is_none());
    }

    #[test]
    fn privileged_ports_lists_the_ports_below_1024_once_and_in_order() {
        let mut a = route("a.example.org", "/", local(1), Scheme::Http);
        a.listen = Some(443);
        let mut b = route("b.example.org", "/", local(2), Scheme::Http);
        b.listen = Some(80);
        let mut c = route("c.example.org", "/", local(3), Scheme::Http);
        c.listen = Some(8080);
        let mut d = route("d.example.org", "/", local(4), Scheme::Http);
        d.listen = Some(80);
        assert_eq!(table(vec![a, b, c, d]).privileged_ports(), vec![80, 443]);
    }

    #[test]
    fn ingress_writes_one_rule_per_service_route_with_its_host_path_and_port() {
        let y = ingress("iyakki-t1", &table(vec![route("app.example.org", "/", svc("web", 8080), Scheme::Http)])).unwrap();
        assert!(y.starts_with("apiVersion: networking.k8s.io/v1\nkind: Ingress\n"));
        assert!(y.contains("  namespace: iyakki-t1\n"));
        assert!(y.contains("    - host: \"app.example.org\"\n"));
        assert!(y.contains("- path: /\n"));
        assert!(y.contains("name: web\n"));
        assert!(y.contains("port: { number: 8080 }"));
        assert!(y.contains("ssl-redirect: \"false\""));
        assert!(y.ends_with("---\n"));
    }

    #[test]
    fn ingress_adds_websocket_timeouts_only_when_a_route_allows_websocket() {
        let on = ingress("ns1", &table(vec![route("app.example.org", "/", svc("web", 80), Scheme::Http)])).unwrap();
        assert!(on.contains("proxy-read-timeout: \"3600\""));
        let mut off_route = route("app.example.org", "/", svc("web", 80), Scheme::Http);
        off_route.websocket = false;
        let off = ingress("ns1", &table(vec![off_route])).unwrap();
        assert!(!off.contains("proxy-read-timeout"));
    }

    #[test]
    fn ingress_lists_each_tls_host_once_from_its_secret_and_turns_on_redirect() {
        let mut a = route("app.example.org", "/", svc("web", 443), Scheme::Https);
        a.cert = Some(CertRef::Secret("app-tls".into()));
        a.redirect_http = true;
        let mut b = route("app.example.org", "/api", svc("api", 8080), Scheme::Https);
        b.cert = Some(CertRef::Secret("app-tls".into()));
        let y = ingress("ns1", &table(vec![a, b])).unwrap();
        assert!(y.contains("  tls:\n    - hosts: [\"app.example.org\"]\n      secretName: app-tls\n"));
        assert_eq!(y.matches("secretName: app-tls").count(), 1);
        assert!(y.contains("ssl-redirect: \"true\""));
    }

    #[test]
    fn ingress_writes_the_ingress_class_when_the_table_names_one() {
        let mut t = table(vec![route("app.example.org", "/", svc("web", 80), Scheme::Http)]);
        t.ingress_class = Some("nginx".into());
        assert!(ingress("ns1", &t).unwrap().contains("  ingressClassName: nginx\n"));
    }

    #[test]
    fn ingress_notes_local_routes_in_a_comment_instead_of_dropping_them_silently() {
        let t = table(vec![route("tools.example.org", "/", local(8081), Scheme::Http), route("app.example.org", "/", svc("web", 80), Scheme::Http)]);
        let y = ingress("ns1", &t).unwrap();
        assert!(y.starts_with("# routes not written here:\n#   tools.example.org/: a local port, served by the local listener\n"));
        assert!(y.contains("name: web"));
    }

    #[test]
    fn ingress_says_no_route_when_every_route_is_left_out_and_explains_why() {
        let mut pass = route("pass.example.org", "/", svc("web", 443), Scheme::Passthrough);
        pass.cert = None;
        let mut vault = route("vault.example.org", "/", svc("web", 443), Scheme::Https);
        vault.cert = Some(CertRef::Vault("app-cert".into()));
        let y = ingress("ns1", &table(vec![pass, vault])).unwrap();
        assert!(y.starts_with("# no route for this cluster\n"));
        assert!(y.contains("TLS passthrough needs the ingress controller's own setting"));
        assert!(y.contains("its certificate is in the vault"));
        assert!(!y.contains("kind: Ingress"));
    }

    #[test]
    fn ingress_refuses_a_namespace_that_is_not_a_name_and_an_invalid_table() {
        let t = table(vec![route("app.example.org", "/", svc("web", 80), Scheme::Http)]);
        assert!(ingress("Bad NS", &t).unwrap_err().contains("is not a name"));
        let bad = table(vec![route("not a host", "/", svc("web", 80), Scheme::Http)]);
        assert!(ingress("ns1", &bad).unwrap_err().contains("is not a host name"));
    }
}

