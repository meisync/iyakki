//! Finding a Gitea that already runs on the dev cluster, for the "Connect Git or Gitea" wizard.
//!
//! Platform services keep a record of the Gitea they installed; an older install, or one made by hand, has none. This asks the cluster
//! itself (one read-only `kubectl get deployments -A -o json`, done off the UI thread by the caller) for a ready Deployment whose name or
//! label says Gitea, and merges it with the record-based list: a record for the same namespace wins, so nothing is listed twice.
//! A found Gitea that this computer cannot use yet (no loopback address, no token in the vault) is listed with the plain reason and
//! one action, the Quick install item "Gitea", which keeps the existing volumes. Nothing here reads a Secret or prints a token.

use crate::gitconn::ClusterGitea;
use crate::pvcguard::{KOut, Kube};

/// The row's title in the wizard.
pub const TITLE: &str = "Gitea on this computer (found on your cluster)";
/// The one action offered for a found Gitea.
pub const ACTION: &str = "Set it up (Quick install: Gitea)";
/// The instance id of a found Gitea has this prefix (it has no record).
pub const FOUND_PREFIX: &str = "found:";

/// A Gitea Deployment of the cluster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub namespace: String,
    pub name: String,
    pub ready: bool,
    /// The port its pod publishes on the node's 127.0.0.1 (a `hostPort` with `hostIP` 127.0.0.1), when it has one.
    pub loopback: Option<u16>,
}

fn says_gitea(d: &serde_json::Value) -> bool {
    let name = d["metadata"]["name"].as_str().unwrap_or("").to_ascii_lowercase();
    if name.contains("gitea") && !name.contains("runner") {
        return true;
    }
    d["metadata"]["labels"].as_object().is_some_and(|l| l.values().any(|v| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("gitea"))))
}

fn loopback_of(d: &serde_json::Value) -> Option<u16> {
    d["spec"]["template"]["spec"]["containers"].as_array()?.iter().filter_map(|c| c["ports"].as_array()).flatten().find_map(|p| {
        let host = p["hostPort"].as_u64()?;
        (p["hostIP"].as_str() == Some("127.0.0.1")).then(|| u16::try_from(host).ok()).flatten()
    })
}

/// The Gitea Deployments in the JSON of `kubectl get deployments -A -o json`, one per namespace (the one named exactly `gitea` first).
pub fn parse(deployments_json: &str) -> Vec<Found> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(deployments_json) else { return Vec::new() };
    let Some(items) = v["items"].as_array() else { return Vec::new() };
    let mut out: Vec<Found> = Vec::new();
    for d in items.iter().filter(|d| says_gitea(d)) {
        let f = Found {
            namespace: d["metadata"]["namespace"].as_str().unwrap_or("").to_string(),
            name: d["metadata"]["name"].as_str().unwrap_or("").to_string(),
            ready: {
                let want = d["spec"]["replicas"].as_u64().unwrap_or(1);
                want > 0 && d["status"]["readyReplicas"].as_u64().unwrap_or(0) >= want
            },
            loopback: loopback_of(d),
        };
        if f.namespace.is_empty() || f.name.is_empty() {
            continue;
        }
        match out.iter().position(|o| o.namespace == f.namespace) {
            Some(i) => {
                if f.name == "gitea" && out[i].name != "gitea" {
                    out[i] = f;
                }
            }
            None => out.push(f),
        }
    }
    out.sort_by(|a, b| a.namespace.cmp(&b.namespace));
    out
}

/// Asks the cluster. Any failure (cluster not running, kubectl missing, bad answer) is an empty list: the wizard then behaves as before.
pub fn discover(kube: Kube<'_>) -> Vec<Found> {
    let o: KOut = kube(&["get", "deployments", "-A", "-o", "json"]);
    if !o.ok {
        return Vec::new();
    }
    parse(&o.stdout)
}

/// What is missing for a found Gitea, in plain words (empty: nothing).
pub fn problem_of(f: &Found) -> String {
    let address = if f.loopback.is_none() { "it has no address on this computer" } else { "" };
    let token = "IYAKKI has no token for it in your vault";
    let tail = if address.is_empty() { token.to_string() } else { format!("{address} and {token}") };
    format!("found in namespace {}, but {tail}. Press the button below to set it up", f.namespace)
}

/// The record-based list plus the ready Gitea Deployments of the cluster that no record covers (a record for the same namespace wins).
pub fn merge(records: Vec<ClusterGitea>, found: &[Found]) -> Vec<ClusterGitea> {
    let mut out = records;
    for f in found.iter().filter(|f| f.ready) {
        if out.iter().any(|r| r.namespace == f.namespace) {
            continue;
        }
        let url = f.loopback.map(|p| format!("http://127.0.0.1:{p}")).unwrap_or_default();
        out.push(ClusterGitea { instance: format!("{FOUND_PREFIX}{}", f.namespace), title: TITLE.to_string(), url, token_vault: String::new(), problem: problem_of(f), namespace: f.namespace.clone(), found: true });
    }
    out
}

#[cfg(test)]
#[path = "gitdiscover_tests.rs"]
mod tests;
