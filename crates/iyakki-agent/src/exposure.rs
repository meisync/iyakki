//! Exposure self-check: which Services of the cluster are open on the LAN, and is each one documented?
//!
//! `kubectl get svc -A -o json` goes in; every Service of type NodePort or LoadBalancer comes out (kube-proxy answers a NodePort on every
//! address of the machine, so a NodePort is a LAN port, and the image's firewall is off by default). Each is compared with
//! [`DOCUMENTED`]. The check only reports: it never changes a Service, a firewall or a setting by itself.
//! The dev cluster's supervisor runs it (`K3sDev`), writes an audit line `devcluster.exposure.unexpected` and raises an inbox warning.

use serde_json::Value;

/// A LAN port that is open on purpose, with where it is documented.
pub struct Documented {
    pub port: u16,
    /// Why it is open, and the document that says so.
    pub why: &'static str,
}

/// The exposed ports that are by design. Anything else a NodePort or LoadBalancer Service opens is reported.
/// The private store always asks for the first free NodePort from 30301 (`PrivateStore::first_node_port`), which is why it is the one port
/// listed. Not listed on purpose: the in-cluster image registry (loopback only, ClusterIP; docs/K3S-DEV.md) and a Gitea deployed with the
/// NodePort exposure (30300, 30222). Gitea is ClusterIP with a loopback port by default now, so a NodePort Gitea is a deliberate choice
/// (or an older install) and is reported.
pub const DOCUMENTED: &[Documented] = &[Documented { port: 30301, why: "the private store, meant for the person's other devices (docs/STORE.md section 13, docs/K3S-DEV.md)" }];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exposed {
    pub namespace: String,
    pub name: String,
    /// `NodePort` or `LoadBalancer`.
    pub kind: String,
    /// The node ports (for a LoadBalancer also the ports it listens on, which kube-proxy opens on the nodes the same way).
    pub ports: Vec<u16>,
}

impl Exposed {
    pub fn line(&self) -> String {
        format!("{}/{} ({}) on port{} {}", self.namespace, self.name, self.kind, if self.ports.len() == 1 { "" } else { "s" }, self.ports.iter().map(u16::to_string).collect::<Vec<_>>().join(", "))
    }
}

/// Every NodePort or LoadBalancer Service in the `kubectl get svc -A -o json` text.
pub fn exposed_services(json: &str) -> Result<Vec<Exposed>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("the Service list was not readable: {e}"))?;
    let items = v["items"].as_array().ok_or("the Service list has no items")?;
    let mut out = Vec::new();
    for it in items {
        let kind = it["spec"]["type"].as_str().unwrap_or("ClusterIP");
        if kind != "NodePort" && kind != "LoadBalancer" {
            continue;
        }
        let mut ports: Vec<u16> = it["spec"]["ports"].as_array().map(|a| a.iter().filter_map(|p| p["nodePort"].as_u64().filter(|n| *n > 0).and_then(|n| u16::try_from(n).ok())).collect()).unwrap_or_default();
        ports.sort_unstable();
        ports.dedup();
        out.push(Exposed { namespace: it["metadata"]["namespace"].as_str().unwrap_or("").into(), name: it["metadata"]["name"].as_str().unwrap_or("").into(), kind: kind.into(), ports });
    }
    out.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    Ok(out)
}

/// What the check found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Open on the LAN and documented.
    pub expected: Vec<Exposed>,
    /// Open on the LAN and not documented (a Service with at least one undocumented port, or a LoadBalancer with no node port).
    pub unexpected: Vec<Exposed>,
}

impl Report {
    /// The same list gives the same text, so the supervisor can raise one warning per change.
    pub fn signature(&self) -> String {
        self.unexpected.iter().map(Exposed::line).collect::<Vec<_>>().join("; ")
    }
}

/// Compare the exposed Services with [`DOCUMENTED`].
pub fn check(json: &str) -> Result<Report, String> {
    let mut r = Report::default();
    for e in exposed_services(json)? {
        let documented = !e.ports.is_empty() && e.ports.iter().all(|p| DOCUMENTED.iter().any(|d| d.port == *p));
        if documented {
            r.expected.push(e);
        } else {
            r.unexpected.push(e);
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(ns: &str, name: &str, kind: &str, ports: &[u16]) -> String {
        let p: Vec<String> = ports.iter().map(|n| format!("{{\"port\":80,\"nodePort\":{n}}}")).collect();
        format!("{{\"metadata\":{{\"namespace\":\"{ns}\",\"name\":\"{name}\"}},\"spec\":{{\"type\":\"{kind}\",\"ports\":[{}]}}}}", p.join(","))
    }
    fn list(items: &[String]) -> String {
        format!("{{\"items\":[{}]}}", items.join(","))
    }

    #[test]
    fn only_nodeport_and_loadbalancer_services_count_and_the_documented_port_is_expected() {
        let j = list(&[svc("default", "kubernetes", "ClusterIP", &[]), svc("iyakki-registry", "registry", "ClusterIP", &[]), svc("iyakki-store", "store", "NodePort", &[30301]), svc("iyakki-gitea", "gitea-http", "NodePort", &[30300]), svc("iyakki-gitea", "gitea-ssh", "NodePort", &[30222]), svc("shop", "web", "LoadBalancer", &[31555])]);
        let r = check(&j).unwrap();
        assert_eq!(r.expected.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["store"]);
        assert_eq!(r.unexpected.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["gitea-http", "gitea-ssh", "web"]);
        assert_eq!(r.unexpected[2].line(), "shop/web (LoadBalancer) on port 31555");
        assert!(r.signature().contains("iyakki-gitea/gitea-ssh (NodePort) on port 30222"));
    }

    #[test]
    fn a_documented_port_next_to_an_undocumented_one_is_unexpected_and_a_nodeless_loadbalancer_too() {
        let r = check(&list(&[svc("a", "mixed", "NodePort", &[30301, 30999]), svc("a", "lb", "LoadBalancer", &[])])).unwrap();
        assert_eq!(r.unexpected.len(), 2);
        assert!(r.expected.is_empty());
        assert_eq!(check(&list(&[svc("a", "clean", "ClusterIP", &[])])).unwrap(), Report::default());
    }

    #[test]
    fn unreadable_input_is_an_error_not_a_clean_bill() {
        assert!(check("not json").is_err());
        assert!(check("{}").is_err());
    }
}
