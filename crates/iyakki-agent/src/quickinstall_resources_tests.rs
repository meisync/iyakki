//! The resources table against the manifests the items really apply, the popup lines and the memory warning. Fakes only.

use super::*;
use resources::*;

/// The numbers as the manifests write them: whole cores in quotes.
fn cpu_text(q: &str) -> String {
    if q.chars().all(|c| c.is_ascii_digit()) { format!("\"{q}\"") } else { q.to_string() }
}

fn manifest_of(item: Item) -> String {
    match item {
        Item::Elasticsearch => elasticsearch_manifest(),
        Item::OpenSearch => opensearch::manifest(),
        Item::ClickHouse => clickhouse::manifest(),
        Item::GraphDb => graphdb_manifest(),
        Item::Prometheus => prometheus_manifest(),
        Item::Matrix => matrix::manifest(),
        Item::SeaweedFs => objstore::manifest(&objstore::SEAWEEDFS),
        Item::RustFs => objstore::manifest(&objstore::RUSTFS),
        Item::Lemonade => lemonade_manifest(),
        Item::Ollama => ollama_cluster::manifest(),
        _ => String::new(),
    }
}

#[test]
fn every_cluster_manifest_carries_cpu_and_memory_requests_and_limits_equal_to_the_table() {
    for r in ALL.iter().filter(|r| !matches!(r.item, Item::Registry | Item::Gitea)) {
        let m = manifest_of(r.item);
        assert!(!m.is_empty(), "{}", r.item.id());
        for (what, want) in [("cpu request", format!("cpu: {}", cpu_text(r.cpu_req))), ("cpu limit", format!("cpu: {}", cpu_text(r.cpu_lim))), ("memory request", format!("memory: {}", r.mem_req)), ("memory limit", format!("memory: {}", r.mem_lim))] {
            assert!(m.contains(&want), "{} lacks its {what} `{want}`", r.item.id());
        }
        assert!(m.contains("limits:") && m.contains("requests:"));
    }
}

#[test]
fn the_registry_and_gitea_manifests_have_their_numbers_too() {
    // both are built elsewhere (regsetup.rs, platform/gitea.rs) as long format strings: their source carries the numbers
    let reg = include_str!("regsetup.rs");
    assert!(reg.contains("cpu: 50m\\n            memory: 64Mi\\n          limits:\\n            cpu: 500m\\n            memory: 512Mi"));
    let gitea = include_str!("platform/gitea.rs");
    assert!(gitea.contains("cpu: 100m\\n            memory: 256Mi\\n          limits:\\n            cpu: \\\"1\\\"\\n            memory: 1Gi"));
    assert_eq!((REGISTRY.cpu_req, REGISTRY.cpu_lim, REGISTRY.mem_req, REGISTRY.mem_lim), ("50m", "500m", "64Mi", "512Mi"));
    assert_eq!((GITEA.cpu_req, GITEA.cpu_lim, GITEA.mem_req, GITEA.mem_lim), ("100m", "1", "256Mi", "1Gi"));
}

#[test]
fn a_limit_is_never_below_its_request_and_the_table_names_each_pod_once() {
    for r in ALL {
        assert!(mem_mb(r.mem_lim) >= mem_mb(r.mem_req) && mem_mb(r.mem_req) > 0, "{}", r.item.id());
        let milli = |q: &str| q.strip_suffix('m').and_then(|n| n.parse::<u32>().ok()).unwrap_or_else(|| q.parse::<u32>().unwrap_or(0) * 1000);
        assert!(milli(r.cpu_lim) >= milli(r.cpu_req) && milli(r.cpu_req) > 0, "{}", r.item.id());
        assert_eq!(cluster_object(r.item).map_or(r.pod, |(_, n)| n), r.pod);
    }
    let mut names: Vec<_> = ALL.iter().map(|r| (r.ns, r.pod)).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), ALL.len());
}

#[test]
fn the_idle_profile_is_what_the_owner_measured_on_2026_11_06() {
    assert_eq!([CLICKHOUSE.idle_mb, OPENSEARCH.idle_mb, ELASTICSEARCH.idle_mb, GRAPHDB.idle_mb, PROMETHEUS.idle_mb, GITEA.idle_mb], [775, 990, 925, 400, 190, 180]);
    for r in [CLICKHOUSE, OPENSEARCH, ELASTICSEARCH, GRAPHDB, PROMETHEUS, GITEA] {
        assert!(r.idle_mb < mem_mb(r.mem_lim), "{} idles below its ceiling", r.item.id());
    }
}

#[test]
fn the_popup_lines_name_the_pod_and_both_numbers_in_plain_words() {
    let l = CLICKHOUSE.lines();
    let all: String = l.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n");
    assert!(all.contains("deployment clickhouse") && all.contains("iyakki-data") && all.contains("clickhouse-"), "{all}");
    assert!(all.contains("asks for 100m (0.1 of a core), may use up to 1 (1 core)"), "{all}");
    assert!(all.contains("asks for 512Mi (512 MB), may use up to 1Gi (1 GB)"), "{all}");
    assert!(all.contains("775 MB") && all.contains("2026-11-06"));
    assert!(!all.to_lowercase().contains("because"), "no claim about why it is busy");
    assert!(ELASTICSEARCH.lines().iter().any(|(_, v)| v.contains("1.5 GB")));
    assert!(!MATRIX.lines().iter().any(|(k, _)| k.starts_with("Idle")), "no idle number where none was measured");
    assert_eq!(cpu_words("2"), "2 cores");
    assert_eq!(cpu_words("500m"), "0.5 of a core");
    assert_eq!(mem_words("16Gi"), "16 GB");
}

#[test]
fn the_review_warns_when_the_services_together_could_pass_sixty_percent_of_the_memory() {
    // 32 GB: 60 % is 19.2 GB. The four databases and Prometheus together stay well under it.
    let few = [Item::K3s, Item::OpenSearch, Item::ClickHouse, Item::Prometheus];
    assert_eq!(total_warning(&few, &[], 32 * 1024), None);
    // both model servers (16 GB each) with the databases do pass it
    let many = [Item::K3s, Item::Lemonade, Item::Ollama, Item::ClickHouse];
    let w = total_warning(&many, &[], 32 * 1024).expect("warns");
    assert!(w.contains("Memory warning") && w.contains("60 %") && w.contains("Ollama 16 GB") && w.contains("32.0 GB"), "{w}");
    // already installed services count
    let w2 = total_warning(&[Item::Ollama], &[Item::Lemonade, Item::OpenSearch], 32 * 1024);
    assert!(w2.is_some_and(|t| t.contains("Lemonade 16 GB") && t.contains("OpenSearch")));
    // a small machine warns for the databases
    assert!(total_warning(&[Item::Elasticsearch, Item::OpenSearch, Item::ClickHouse], &[], 6 * 1024).is_some());
    // unknown memory, or nothing from the table: no warning
    assert_eq!(total_warning(&many, &[], 0), None);
    assert_eq!(total_warning(&[Item::Node, Item::K3s], &[Item::Lemonade], 4 * 1024), None);
}
