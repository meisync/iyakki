//! The independent checks that run before a Quick install marker is written (`StepEnv::verify`): which Deployment and Service an item is,
//! and the plain readings of what `kubectl` printed. The commands themselves are run by `RealEnv`; this file only decides.

use super::{clickhouse, gitea, matrix, objstore, opensearch, Item, LEMONADE_NAMESPACE, NAMESPACE, PROM_NAMESPACE};

/// `(namespace, Deployment, Service)` of an item that runs as one Deployment behind one Service in the cluster.
pub fn target(item: Item) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match item {
        Item::Elasticsearch => (NAMESPACE, "elasticsearch", "elasticsearch"),
        Item::GraphDb => (NAMESPACE, "graphdb", "graphdb"),
        Item::Prometheus => (PROM_NAMESPACE, "prometheus", "prometheus"),
        Item::OpenSearch => (opensearch::NS, "opensearch", "opensearch"),
        Item::ClickHouse => (clickhouse::NS, "clickhouse", "clickhouse"),
        Item::Matrix => (matrix::NS, "matrix", "matrix"),
        Item::SeaweedFs => (objstore::NS, "seaweedfs", "seaweedfs"),
        Item::RustFs => (objstore::NS, "rustfs", "rustfs"),
        Item::Gitea => (gitea::NAMESPACE, "gitea", "gitea-http"),
        Item::Ollama => (LEMONADE_NAMESPACE, "ollama", "ollama"),
        Item::Lemonade => (LEMONADE_NAMESPACE, "lemonade", "lemonade"),
        _ => return None,
    })
}

/// Reads `kubectl get deployment -o jsonpath='{.spec.replicas} {.status.readyReplicas}'`: at least one replica wanted and all of them ready.
pub fn ready_from(text: &str) -> Result<(u32, u32), String> {
    let mut w = text.split_whitespace();
    let want: u32 = w.next().and_then(|v| v.parse().ok()).unwrap_or(1);
    // a Deployment with no ready replica prints no second number
    let ready: u32 = w.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    if want == 0 {
        return Err("it is scaled to zero, nothing runs".into());
    }
    if ready < want {
        return Err(format!("{ready} of {want} pod(s) are Ready"));
    }
    Ok((want, ready))
}

/// Reads `kubectl get endpoints <service> -o jsonpath='{.subsets[*].addresses[*].ip}'`: the Service routes to at least one Ready pod.
pub fn endpoints_from(ok: bool, text: &str) -> Result<usize, String> {
    if !ok {
        return Err("the Service does not exist".into());
    }
    let n = text.split_whitespace().count();
    if n == 0 { Err("the Service has no address behind it (no Ready pod)".into()) } else { Ok(n) }
}

/// The note a passed deployment check leaves in the marker.
pub fn deployment_note(deploy: &str, service: &str, pods: u32, addresses: usize) -> String {
    format!("verified: {deploy} has {pods} Ready pod(s) and the Service {service} answers with {addresses} address(es)")
}
