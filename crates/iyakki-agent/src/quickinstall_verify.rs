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
        Item::Registry => (crate::regsetup::IN_NS, crate::regsetup::IN_NAME, crate::regsetup::IN_NAME),
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

/// Reads `kubectl get nodes -o jsonpath` of every node's Ready condition (`True True`): at least one node and none that is not Ready.
pub fn nodes_ready_from(text: &str) -> Result<usize, String> {
    let all: Vec<&str> = text.split_whitespace().collect();
    if all.is_empty() {
        return Err("the cluster lists no node".into());
    }
    let bad = all.iter().filter(|c| **c != "True").count();
    if bad > 0 {
        return Err(format!("{bad} of {} node(s) are not Ready", all.len()));
    }
    Ok(all.len())
}

/// The note for a sandbox that is listed by name (the Linux apps items), or why it is not.
pub fn sandbox_note(name: &str, listed: Result<(), String>) -> Result<String, String> {
    listed.map(|()| format!("verified: the sandbox {name} is listed")).map_err(|e| format!("the sandbox {name} is not there: {e}"))
}

/// The voice pack: Lemonade itself lists both models as downloaded (asked again, after the pack's own self-test).
pub fn voice_pack_note(downloaded: &[String]) -> Result<String, String> {
    let missing: Vec<&str> = [crate::voicepack::WHISPER_MODEL, crate::voicepack::KOKORO_MODEL].into_iter().filter(|m| !downloaded.iter().any(|d| d == m)).collect();
    if missing.is_empty() {
        Ok(format!("verified: Lemonade lists {} and {} as downloaded", crate::voicepack::WHISPER_MODEL, crate::voicepack::KOKORO_MODEL))
    } else {
        Err(format!("Lemonade does not list {} as downloaded", missing.join(" and ")))
    }
}

/// The Mac firmware: the run said it installed, and every pack it names has its record with each file present in the firmware folder.
/// `packs` holds, per pack the run reported files for, `(pack id, files the run reported, the record's files or None, how many of them are in the firmware folder)`.
pub fn mac_firmware_note(installed: bool, packs: &[(&str, usize, Option<usize>, usize)]) -> Result<String, String> {
    if !installed {
        return Err("the run did not record an installed result".into());
    }
    let mut parts = Vec::new();
    for (id, reported, record, present) in packs {
        if *reported == 0 {
            continue;
        }
        let Some(n) = record else { return Err(format!("the pack {id} has no record on this machine")) };
        if *n == 0 || present < n {
            return Err(format!("the pack {id} lists {n} file(s) and {present} are in the firmware folder"));
        }
        parts.push(format!("{id} ({n} file(s))"));
    }
    if parts.is_empty() {
        Ok("verified: the run installed nothing because Apple's macOS had no extra files for this Mac".into())
    } else {
        Ok(format!("verified: {} recorded with every file in the firmware folder", parts.join(", ")))
    }
}
