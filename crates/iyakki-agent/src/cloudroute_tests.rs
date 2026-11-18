//! The route table and the certificate checks (moved with the module to iyakki-shared; the tests stay where rustls is).


use crate::cloudroute::*;
use iyakki_shared::cloudroute::{Access, Scheme, Target};

fn svc(host: &str, path: &str, name: &str) -> Route {
    Route { host: host.into(), path_prefix: path.into(), target: Target::Service { name: name.into(), port: 8080 }, scheme: Scheme::Http, listen: None, websocket: true, redirect_http: false, cert: None, access: Access::Members, client_ca: None }
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
