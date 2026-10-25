use super::*;
use crate::aiprofile::{self, rule, visibility, Vis};
use crate::k8s::FakeKube;
use crate::quickinstall::Item;

fn node(name: &str, labels: &str, res: &str) -> String {
    format!(r#"{{"metadata":{{"name":"{name}","labels":{{{labels}}}}},"status":{{"allocatable":{{{res}}},"capacity":{{{res}}}}}}}"#)
}

fn list(nodes: &[String]) -> String {
    format!(r#"{{"items":[{}]}}"#, nodes.join(","))
}

fn kube(json: &str) -> FakeKube {
    let k = FakeKube::new();
    k.answer("get nodes", json);
    k
}

fn target() -> Target {
    Target { kubeconfig: "/fake/kubeconfig".into(), context: "remote".into(), ..Default::default() }
}

fn probe_json(json: &str) -> RemoteProfile {
    probe(&kube(json), &target(), "lab", 100).unwrap()
}

const NV4090: &str = r#""nvidia.com/gpu.product":"NVIDIA-GeForce-RTX-4090","nvidia.com/gpu.memory":"24564""#;

#[test]
fn a_cluster_without_gpu_nodes_has_no_gpu() {
    let p = probe_json(&list(&[node("a", "", r#""cpu":"8","memory":"32Gi""#), node("b", "", r#""nvidia.com/gpu":"0""#)]));
    assert!(p.gpus.is_empty() && !p.capable() && !p.unknown());
    assert_eq!(p.words(), "no GPU");
    assert_eq!(probe_json(r#"{"items":[]}"#).gpus.len(), 0);
}

#[test]
fn an_nvidia_node_with_labels_gives_name_count_and_memory() {
    let p = probe_json(&list(&[node("gpu1", NV4090, r#""nvidia.com/gpu":"2""#)]));
    assert_eq!(p.gpus.len(), 1);
    assert_eq!((p.gpus[0].name.as_str(), p.gpus[0].count), ("NVIDIA GeForce RTX 4090", 2));
    assert!((p.gpus[0].mem_gb.unwrap() - 23.99).abs() < 0.01);
    assert!(p.capable());
    assert!((p.usable_gb() - 22.99).abs() < 0.01);
    assert_eq!(p.words(), "2 x NVIDIA GeForce RTX 4090 (24 GB each)");
}

#[test]
fn an_amd_node_and_gpus_without_memory_labels() {
    let amd = probe_json(&list(&[node("amd1", r#""amd.com/gpu.vram":"16G","amd.com/gpu.product-name":"Radeon-RX-7800-XT""#, r#""amd.com/gpu":"1""#)]));
    assert_eq!(amd.gpus[0].vendor, Vendor::Amd);
    assert_eq!(amd.gpus[0].mem_gb, Some(16.0));
    assert_eq!(amd.words(), "1 x AMD Radeon RX 7800 XT (16 GB)");
    assert!(amd.capable());
    let none = probe_json(&list(&[node("g", "", r#""nvidia.com/gpu":"1""#)]));
    assert!(none.unknown() && !none.capable());
    assert_eq!(none.words(), "1 x NVIDIA GPU (GPU found, memory unknown)");
}

#[test]
fn several_nodes_are_merged_and_the_best_gpu_wins() {
    let p = probe_json(&list(&[
        node("a", r#""nvidia.com/gpu.product":"Tesla-T4","nvidia.com/gpu.memory":"15360""#, r#""nvidia.com/gpu":"1""#),
        node("b", NV4090, r#""nvidia.com/gpu":"1""#),
        node("c", NV4090, r#""nvidia.com/gpu":"1""#),
        node("d", "", r#""cpu":"4""#),
    ]));
    assert_eq!(p.gpus.len(), 2);
    assert_eq!(p.best().unwrap().count, 2, "the two 4090s are one kind");
    assert_eq!(p.words(), "2 x NVIDIA GeForce RTX 4090 (24 GB each)");
    assert_eq!(parse_mem_label("24G"), Some(24.0));
    assert_eq!(parse_mem_label("8192Mi"), Some(8.0));
    assert_eq!(parse_mem_label("24564"), Some(24564.0 / 1024.0));
    assert_eq!(parse_mem_label("lots"), None);
    assert_eq!(parse_mem_label("0"), None);
}

#[test]
fn garbage_json_never_panics_and_failures_say_why() {
    for j in ["", "null", "[]", "{", r#"{"items":"x"}"#, r#"{"items":[1,null,{"status":5}]}"#, r#"{"items":[{"status":{"allocatable":{"nvidia.com/gpu":"99999999999999"}}}]}"#, r#"{"items":[{"status":{"capacity":{"amd.com/gpu":-3}}}]}"#] {
        let _ = parse_nodes(j);
    }
    let k = FakeKube::new();
    k.fail("get nodes", "Unable to connect to the server: dial tcp: i/o timeout\nmore");
    assert_eq!(probe(&k, &target(), "lab", 1).unwrap_err(), "Unable to connect to the server: dial tcp: i/o timeout");
    let k = FakeKube::new();
    k.missing();
    assert_eq!(probe(&k, &target(), "lab", 1).unwrap_err(), "kubectl is not installed");
    assert!(probe(&kube("not json"), &target(), "lab", 1).is_err());
    // read-only: only `get nodes`
    let k = kube(&list(&[]));
    probe(&k, &target(), "lab", 1).unwrap();
    assert_eq!(k.calls().len(), 1);
    assert!(k.calls()[0].ends_with("get nodes -o json") && k.calls()[0].contains("--kubeconfig /fake/kubeconfig --context remote"));
}

#[test]
fn a_failing_cluster_keeps_the_last_value_stale_and_backs_off() {
    let c = Cache::new();
    let t = target();
    assert!(c.get(&t).is_none());
    c.refresh(&kube(&list(&[node("g", NV4090, r#""nvidia.com/gpu":"1""#)])), &t, "lab", 1000);
    let p = c.get(&t).unwrap();
    assert!(!p.stale && p.capable());
    // not due again until REFRESH_SECS later
    let k2 = kube(&list(&[]));
    c.refresh(&k2, &t, "lab", 1000 + REFRESH_SECS - 1);
    assert!(k2.calls().is_empty(), "no call before it is due");
    // it fails: last value kept, marked stale, with the reason
    let bad = FakeKube::new();
    bad.fail("get nodes", "connection refused");
    c.refresh(&bad, &t, "lab", 1000 + REFRESH_SECS);
    let s = c.get(&t).unwrap();
    assert!(s.stale && s.capable() && s.error == "connection refused", "{s:?}");
    assert_eq!(bad.calls().len(), 1);
    // the back-off doubles: 30 s, 60 s, ... up to 600 s
    let mut now = 1000 + REFRESH_SECS;
    let mut waits = vec![];
    for _ in 0..7 {
        let before = bad.calls().len();
        // find the first second it asks again
        let mut w = 0;
        while bad.calls().len() == before {
            w += 1;
            c.refresh(&bad, &t, "lab", now + w);
            assert!(w < 2000);
        }
        now += w;
        waits.push(w);
    }
    assert_eq!(waits, vec![30, 60, 120, 240, 480, 600, 600], "30 s after the first failure, doubled each time, capped at 600");
    // it answers again: fresh value, no longer stale
    c.refresh(&kube(&list(&[])), &t, "lab", now + 601);
    let f = c.get(&t).unwrap();
    assert!(!f.stale && f.gpus.is_empty());
    // another cluster has its own entry
    assert!(c.get(&Target { context: "other".into(), ..t.clone() }).is_none());
    // a failure before any answer: an empty stale profile, never "capable"
    let c2 = Cache::new();
    c2.refresh(&bad, &t, "lab", 5);
    let e = c2.get(&t).unwrap();
    assert!(e.stale && e.gpus.is_empty() && !e.capable());
}

fn none_local() -> aiprofile::AiProfile {
    let r = std::env::temp_dir().join(format!("iyakki-airm-{}-{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(r.join("proc")).unwrap();
    std::fs::write(r.join("proc/meminfo"), format!("MemTotal: {} kB\n", 8 * 1024 * 1024)).unwrap();
    let p = aiprofile::detect(&r);
    let _ = std::fs::remove_dir_all(&r);
    p
}

fn gpu_local() -> aiprofile::AiProfile {
    let r = std::env::temp_dir().join(format!("iyakki-airg-{}-{:?}", std::process::id(), std::thread::current().id()));
    let d = r.join("sys/bus/pci/devices/0000:01:00.0");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::create_dir_all(r.join("proc")).unwrap();
    std::fs::write(r.join("proc/meminfo"), format!("MemTotal: {} kB\n", 32 * 1024 * 1024)).unwrap();
    for (f, c) in [("class", "0x030000"), ("vendor", "0x10de"), ("device", "0x2503")] {
        std::fs::write(d.join(f), c).unwrap();
    }
    let p = aiprofile::detect(&r);
    let _ = std::fs::remove_dir_all(&r);
    p
}

#[test]
fn local_none_with_a_remote_gpu_shows_the_model_servers_and_says_why() {
    let remote = probe_json(&list(&[node("gpu1", NV4090, r#""nvidia.com/gpu":"2""#)]));
    let p = none_local().with_remote(Some(remote));
    for i in [Item::Lemonade, Item::Ollama] {
        assert_eq!(rule(&p, i), Vis::ShownBecause("your cluster 'lab' has 2 x NVIDIA GeForce RTX 4090 (24 GB each)".into()), "{i:?}");
        assert!(visibility(&p, i, false).shown());
    }
    // the installs on THIS machine stay tied to its own hardware
    assert!(matches!(rule(&p, Item::GpuVulkan), Vis::Hidden(_)));
    assert!(matches!(rule(&p, Item::GpuNvidia), Vis::Hidden(_)));
    // a Mac host is optional when the cluster has a GPU
    assert!(matches!(rule(&p, Item::MacHost), Vis::NotRecommended(r) if r.contains("your cluster 'lab'")));
    assert!(p.summary().contains("Cluster: 'lab' has 2 x NVIDIA GeForce RTX 4090 (24 GB each)"), "{}", p.summary());
    // a stale value is said to be stale
    let mut stale = probe_json(&list(&[node("g", NV4090, r#""nvidia.com/gpu":"1""#)]));
    stale.stale = true;
    let q = none_local().with_remote(Some(stale));
    assert!(rule(&q, Item::Lemonade).reason().unwrap().contains("last known"));
    // GPU found, memory unknown: shown
    let unk = none_local().with_remote(Some(probe_json(&list(&[node("g", "", r#""nvidia.com/gpu":"1""#)]))));
    assert!(rule(&unk, Item::Ollama).reason().unwrap().contains("memory unknown"));
}

#[test]
fn local_gpu_with_a_remote_without_one_changes_nothing() {
    let base = gpu_local();
    let with = gpu_local().with_remote(Some(probe_json(&list(&[node("a", "", r#""cpu":"8""#)]))));
    for i in Item::ALL {
        assert_eq!(rule(&base, i), rule(&with, i), "{i:?}");
    }
    // and a small remote GPU does not bring tools back for a machine without one
    let small = none_local().with_remote(Some(probe_json(&list(&[node("g", r#""nvidia.com/gpu.product":"GTX-1050","nvidia.com/gpu.memory":"2048""#, r#""nvidia.com/gpu":"1""#)]))));
    assert!(matches!(rule(&small, Item::Lemonade), Vis::Hidden(_)));
    // no remote at all: as before
    assert!(matches!(rule(&none_local(), Item::Lemonade), Vis::Hidden(_)));
}

#[test]
fn only_a_remote_cluster_that_answers_is_read() {
    use crate::devcluster::{ClusterProfile, Source};
    let mut p = ClusterProfile { name: "lab".into(), kubeconfig: "/k".into(), context: "c".into(), server: String::new(), source: Source::Vault, exists: true, readable: true, tested: true, reachable: true, k3s: true, local: false, version: String::new(), can_i: String::new(), problem: String::new(), fix: String::new() };
    let (t, name) = target_of(&p).unwrap();
    assert_eq!((t.kubeconfig.as_str(), t.context.as_str(), name.as_str()), ("/k", "c", "lab"));
    p.local = true;
    assert!(target_of(&p).is_none(), "this machine's own cluster is local hardware");
    p.local = false;
    p.reachable = false;
    assert!(target_of(&p).is_none());
}
