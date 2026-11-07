//! Routing by domain name, and the certificates of HTTPS routes (docs/development/CLOUD-MULTITENANT-PLAN.md, section 3c).
//!
//! [`RouteTable`] is the list of routes (host, path prefix, target, scheme); [`RouteTable::pick`] chooses one for a request's `Host` header
//! and path; [`ingress`] writes the Kubernetes Ingress of a table. [`check_certificate`] says whether a certificate chain and key a person
//! supplied can serve a host (the key matches, the host is covered, it is not expired, and when it runs out), with no X509 library: a
//! small reader of the few fields it needs ([`read_cert`]).
//!
//! The local listener that applies a table (sockets, SNI, WebSocket upgrade) is `cloudlisten.rs`.

use serde::{Deserialize, Serialize};

use crate::cloudtenant::{dns_label, dns_subdomain, host_name};

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

fn wildcard_or_host(h: &str) -> bool {
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
                if !crate::cloudstorage::plain_name(n) {
                    return Err(format!("route {}: \"{n}\" is not a vault name", r.host));
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

/// A certificate chain and private key (PEM), for `host`, at `now` (Unix seconds). Returns what it found and the line to show (a warning
/// when it runs out within [`WARN_DAYS`]).
pub fn check_certificate(chain_pem: &str, key_pem: &str, host: &str, now: i64) -> Result<(CertInfo, String), String> {
    use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(chain_pem.as_bytes()).collect::<Result<_, _>>().map_err(|e| format!("the certificate is not PEM: {e}"))?;
    let Some(first) = chain.first() else { return Err("no certificate found in the file (expected -----BEGIN CERTIFICATE-----)".into()) };
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|_| "the private key is not PEM (expected -----BEGIN PRIVATE KEY-----)".to_string())?;
    let signer = rustls::crypto::ring::sign::any_supported_type(&key).map_err(|_| "the private key type is not supported (use RSA, ECDSA P-256/P-384 or Ed25519)".to_string())?;
    let certified = rustls::sign::CertifiedKey::new(chain.clone(), signer);
    certified.keys_match().map_err(|_| "the private key does not belong to this certificate".to_string())?;
    let info = read_cert(first.as_ref()).ok_or("the certificate could not be read")?;
    if info.names.is_empty() {
        return Err("the certificate has no Subject Alternative Names; browsers need them".into());
    }
    if !info.names.iter().any(|n| name_covers(n, host)) {
        return Err(format!("the certificate covers {} and not {host}", info.names.join(", ")));
    }
    if now < info.not_before {
        return Err("the certificate is not valid yet".into());
    }
    if now >= info.not_after {
        return Err("the certificate has expired".into());
    }
    let days = (info.not_after - now) / 86400;
    let line = if days <= WARN_DAYS { format!("valid, but runs out in {days} day(s): renew it soon") } else { format!("valid for {days} more days") };
    Ok((info, line))
}

/// The chain and key as something rustls can serve with (the listener, `cloudlisten.rs`). Call [`check_certificate`] first: this only
/// parses. The errors never contain key material.
pub fn load_certified(chain_pem: &str, key_pem: &str) -> Result<rustls::sign::CertifiedKey, String> {
    use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(chain_pem.as_bytes()).collect::<Result<_, _>>().map_err(|e| format!("the certificate is not PEM: {e}"))?;
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|_| "the private key is not PEM".to_string())?;
    let signer = rustls::crypto::ring::sign::any_supported_type(&key).map_err(|_| "the private key type is not supported".to_string())?;
    Ok(rustls::sign::CertifiedKey::new(chain, signer))
}

/// A self-signed certificate and key for a local trial: (chain PEM, key PEM). Marked as such by the caller; browsers will not trust it.
pub fn self_signed(host: &str) -> Result<(String, String), String> {
    if !wildcard_or_host(host) && host != "localhost" {
        return Err(format!("\"{host}\" is not a host name"));
    }
    let c = rcgen::generate_simple_self_signed(vec![host.to_string()]).map_err(|e| e.to_string())?;
    Ok((c.cert.pem(), c.key_pair.serialize_pem()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(host: &str, path: &str, name: &str) -> Route {
        Route { host: host.into(), path_prefix: path.into(), target: Target::Service { name: name.into(), port: 8080 }, scheme: Scheme::Http, listen: None, websocket: true, redirect_http: false, cert: None, access: Access::Members }
    }
    fn table(r: Vec<Route>) -> RouteTable {
        RouteTable { routes: r, ingress_class: None }
    }

    #[test]
    fn the_exact_host_beats_a_wildcard_and_the_longest_path_wins() {
        let t = table(vec![svc("*.example.org", "/", "wild"), svc("app.example.org", "/", "app"), svc("app.example.org", "/api", "api")]);
        let n = |h: &str, p: &str| match t.pick(h, p, 80).map(|r| &r.target) {
            Some(Target::Service { name, .. }) => name.clone(),
            _ => "none".into(),
        };
        assert_eq!(n("app.example.org", "/"), "app");
        assert_eq!(n("APP.example.org:80", "/x"), "app", "case and a port are ignored");
        assert_eq!(n("app.example.org", "/api/v1"), "api");
        assert_eq!(n("app.example.org", "/apix"), "app", "/api is not a prefix of /apix");
        assert_eq!(n("web.example.org", "/"), "wild");
        assert_eq!(n("a.b.example.org", "/"), "none", "a wildcard is one label");
        assert_eq!(n("example.org", "/"), "none");
        assert_eq!(n("other.test", "/"), "none");
        assert!(t.pick("app.example.org", "/", 8080).is_none(), "another listening port is another route set");
    }

    #[test]
    fn a_table_is_checked_for_names_ports_certificates_and_duplicates() {
        let mut https = svc("a.example.org", "/", "s");
        https.scheme = Scheme::Https;
        assert!(table(vec![https.clone()]).check(&[]).unwrap_err().contains("needs a certificate"));
        https.cert = Some(CertRef::Secret("a-tls".into()));
        assert!(table(vec![https.clone()]).check(&[]).is_ok());
        assert!(table(vec![https.clone()]).check(&[443]).unwrap_err().contains("already used"));
        assert!(table(vec![https.clone(), https.clone()]).check(&[]).unwrap_err().contains("twice"));
        let mut http_with_cert = svc("a.example.org", "/", "s");
        http_with_cert.cert = Some(CertRef::Secret("x".into()));
        assert!(table(vec![http_with_cert]).check(&[]).is_err());
        for bad in ["a b.example.org", "a.example.org\nx: y", "localhost", "*.*.example.org", ""] {
            assert!(table(vec![svc(bad, "/", "s")]).check(&[]).is_err(), "{bad:?}");
        }
        assert!(table(vec![svc("a.example.org", "api", "s")]).check(&[]).is_err(), "path must start with /");
        assert!(table(vec![svc("a.example.org", "/a b", "s")]).check(&[]).is_err());
        assert_eq!(table(vec![https, svc("b.example.org", "/", "s")]).privileged_ports(), vec![80, 443]);
        let mut r8 = svc("b.example.org", "/", "s");
        r8.listen = Some(8443);
        assert!(table(vec![r8]).privileged_ports().is_empty());
    }

    #[test]
    fn the_ingress_has_a_rule_per_route_tls_per_host_and_says_what_it_left_out() {
        let mut a = svc("app.example.org", "/", "iyakki");
        a.scheme = Scheme::Https;
        a.cert = Some(CertRef::Secret("app-tls".into()));
        a.redirect_http = true;
        let mut local = svc("dev.example.org", "/", "x");
        local.target = Target::Local { port: 3000 };
        let t = RouteTable { routes: vec![a, svc("api.example.org", "/v1", "api"), local], ingress_class: Some("nginx".into()) };
        let y = ingress("iyakki-acme", &t).unwrap();
        assert!(y.contains("kind: Ingress") && y.contains("namespace: iyakki-acme") && y.contains("ingressClassName: nginx"), "{y}");
        assert!(y.contains("host: \"app.example.org\"") && y.contains("host: \"api.example.org\"") && y.contains("path: /v1"), "{y}");
        assert!(y.contains("secretName: app-tls") && y.contains("hosts: [\"app.example.org\"]"), "{y}");
        assert!(y.contains("proxy-read-timeout") && y.contains("ssl-redirect: \"true\""), "{y}");
        assert!(y.contains("dev.example.org/: a local port") && !y.contains("host: \"dev.example.org\""), "{y}");
        assert!(ingress("iyakki-acme", &table(vec![])).unwrap().contains("no route for this cluster"));
    }

    #[test]
    fn a_self_signed_certificate_is_read_and_checked_for_host_key_and_dates() {
        let (chain, key) = self_signed("app.example.org").unwrap();
        let now = 1_800_000_000; // 2027-01
        let (info, line) = check_certificate(&chain, &key, "app.example.org", now).unwrap();
        assert!(info.names.contains(&"app.example.org".to_string()), "{info:?}");
        assert!(line.starts_with("valid"), "{line}");
        assert!(info.not_before < info.not_after);
        // another host
        assert!(check_certificate(&chain, &key, "other.example.org", now).unwrap_err().contains("covers app.example.org and not other.example.org"));
        // a key of another certificate
        let (_, other_key) = self_signed("app.example.org").unwrap();
        assert!(check_certificate(&chain, &other_key, "app.example.org", now).unwrap_err().contains("does not belong"));
        // dates: before it starts and after it ends, and the warning near the end
        assert!(check_certificate(&chain, &key, "app.example.org", info.not_before - 1).unwrap_err().contains("not valid yet"));
        assert!(check_certificate(&chain, &key, "app.example.org", info.not_after).unwrap_err().contains("expired"));
        let near = info.not_after - 5 * 86400;
        assert!(check_certificate(&chain, &key, "app.example.org", near).unwrap().1.contains("runs out in 5 day"));
        // not PEM, no certificate, no key
        assert!(check_certificate("hello", &key, "app.example.org", now).unwrap_err().contains("no certificate"));
        assert!(check_certificate(&chain, "hello", "app.example.org", now).unwrap_err().contains("private key"));
    }

    #[test]
    fn name_matching_follows_the_wildcard_rule_and_the_calendar_is_right() {
        assert!(name_covers("*.example.org", "a.example.org") && name_covers("A.Example.org", "a.example.org"));
        assert!(!name_covers("*.example.org", "a.b.example.org") && !name_covers("*.example.org", "example.org") && !name_covers("*.example.org", ".example.org"));
    }
}
