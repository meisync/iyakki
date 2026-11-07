//! Ollama in the cluster with fakes only: no cluster, no network, no host program. The item's identity, the manifest, where it runs, the
//! move decision and review, the job in its three shapes, and that a refused move changes nothing.

use super::*;
use ollama_cluster as oc;

struct Fake {
    lines: Vec<String>,
    calls: Vec<String>,
    manifests: Vec<String>,
    marks: Vec<(Item, String)>,
    afters: Vec<String>,
    mode: Option<oc::Mode>,
    host: oc::HostInfo,
    copy_fails: bool,
    /// What the cluster Ollama lists (answers after `answers_after` asks).
    cluster_models: Vec<String>,
    host_models: Result<Vec<String>, String>,
    answers_after: u32,
    asked: u32,
}

impl Fake {
    fn new() -> Fake {
        Fake { lines: vec![], calls: vec![], manifests: vec![], marks: vec![], afters: vec![], mode: None, host: oc::HostInfo::default(), copy_fails: false, cluster_models: vec![], host_models: Err("host is not asked".into()), answers_after: 0, asked: 0 }
    }
}

impl StepEnv for Fake {
    fn say(&mut self, line: &str) {
        self.lines.push(line.into());
    }
    fn dep_state(&mut self, _dep: Item) -> DepState {
        DepState::Ready
    }
    fn k3s_present(&mut self) -> bool {
        true
    }
    fn k3s_elsewhere(&mut self) -> Option<String> {
        None
    }
    fn install_k3s(&mut self) -> Result<String, String> {
        Err("not used".into())
    }
    fn request_k3s_start(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn cluster_ready(&mut self) -> bool {
        true
    }
    fn install_cli(&mut self, _id: &str) -> Result<String, String> {
        Err("the CLI catalogue must not be used for the cluster Ollama".into())
    }
    fn kubectl_apply(&mut self, name: &str, manifest: &str) -> Result<String, String> {
        self.calls.push(format!("apply {name}"));
        self.manifests.push(manifest.into());
        Ok("deployment.apps/ollama configured".into())
    }
    fn rollout(&mut self, ns: &str, name: &str, _t: Duration) -> Result<(), String> {
        self.calls.push(format!("rollout {ns}/{name}"));
        Ok(())
    }
    fn sleep(&mut self, _d: Duration) {}
    fn mark(&mut self, item: Item, detail: &str) -> Result<(), String> {
        self.marks.push((item, detail.into()));
        Ok(())
    }
    fn request_after(&mut self, what: &str) -> Result<(), String> {
        self.afters.push(what.into());
        Ok(())
    }
    fn lemonade_ready(&mut self) -> bool {
        false
    }
    fn lemonade_pull(&mut self, _model: &str) -> Result<(), String> {
        Err("not used".into())
    }
    fn ollama_mode(&mut self) -> Option<oc::Mode> {
        self.mode
    }
    fn ollama_host(&mut self) -> oc::HostInfo {
        self.host.clone()
    }
    fn ollama_copy(&mut self, dir: &Path) -> Result<String, String> {
        self.calls.push(format!("copy {}", dir.display()));
        if self.copy_fails { Err("the copy failed (tar: no space left); the host Ollama and its models were not touched".into()) } else { Ok("the models were copied".into()) }
    }
    fn ollama_models(&mut self, cluster: bool) -> Result<Vec<String>, String> {
        if cluster {
            self.asked += 1;
            if self.asked <= self.answers_after { Err("connection refused".into()) } else { Ok(self.cluster_models.clone()) }
        } else {
            self.host_models.clone()
        }
    }
}

fn host(bytes: u64, free: Option<u64>) -> oc::HostInfo {
    oc::HostInfo { models_dir: PathBuf::from("/data/ollama/models"), models_bytes: bytes, free_bytes: free }
}

const GB: u64 = 1 << 30;

#[test]
fn the_item_keeps_its_identity_and_now_needs_the_cluster() {
    assert_eq!(Item::parse("ollama"), Some(Item::Ollama));
    assert_eq!(Item::Ollama.id(), "ollama");
    assert_eq!(Item::Ollama.title(), "Ollama");
    assert_eq!(Item::Ollama.group(), Group::LocalModels);
    assert_eq!(Item::Ollama.needs(), &[Item::K3s]);
    assert_eq!(order(Item::Ollama, &|_| false), vec![Item::K3s, Item::Ollama]);
    assert!(Item::Ollama.is_service() && Item::Ollama.runs_as_job());
    // the host install is still found through the catalogue entry
    assert_eq!(Item::Ollama.cli_id(), Some("ollama"));
    assert!(Item::Ollama.licence().contains("MIT") && Item::Ollama.licence().contains("not shipped"));
    assert_eq!(cluster_object(Item::Ollama), Some(("iyakki-ai", "ollama")));
}

#[test]
fn the_manifest_is_a_pinned_unprivileged_limited_loopback_only_pod_with_a_kept_claim() {
    let m = oc::manifest();
    let images: Vec<&str> = m.lines().filter_map(|l| l.trim().strip_prefix("image: ")).collect();
    assert_eq!(images.len(), 2, "the init container and the server use the one image");
    assert!(images.iter().all(|i| *i == oc::image()));
    assert!(oc::image().starts_with("docker.io/ollama/ollama:") && !oc::image().contains("latest"));
    if !oc::DIGEST.is_empty() {
        assert!(oc::DIGEST.strip_prefix("sha256:").is_some_and(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())));
    } else {
        assert!(!oc::image().contains('@'), "no digest is invented");
    }
    for want in ["kind: Deployment", "kind: Service", "kind: PersistentVolumeClaim", "name: ollama-data", "storage: 30Gi", "type: ClusterIP", "namespace: iyakki-ai", "hostIP: 127.0.0.1", "hostPort: 11436", "readinessProbe", "runAsNonRoot: true", "runAsUser: 10001", "allowPrivilegeEscalation: false", "drop: [ALL]", "automountServiceAccountToken: false", "type: RuntimeDefault", "OLLAMA_MODELS", "claimName: ollama-data", "strategy:\n    type: Recreate"] {
        assert!(m.contains(want), "missing {want}");
    }
    for bad in ["privileged: true", "hostPath", "hostNetwork", "NodePort", "LoadBalancer", "SYS_ADMIN", "hostIPC", "hostPID"] {
        assert!(!m.contains(bad), "{bad}");
    }
    // the only root part is the init container that hands the volume over, with only the capabilities that needs
    assert_eq!(m.matches("runAsUser: 0").count(), 1);
    // no GPU is requested by the plain manifest: the GPU stages patch it the way they patch Lemonade
    assert!(!m.contains("nvidia.com/gpu") && !m.contains("/dev/dri") && !m.contains("OLLAMA_VULKAN"));
    // the same numbers as the table (CPU and memory requests and limits)
    let r = resources::OLLAMA;
    for want in [format!("cpu: {}", r.cpu_req), format!("memory: {}", r.mem_req), format!("cpu: \"{}\"", r.cpu_lim), format!("memory: {}", r.mem_lim)] {
        assert!(m.contains(&want), "missing {want}");
    }
}

#[test]
fn the_gpu_stages_patch_ollama_the_way_they_patch_lemonade() {
    use crate::gpu_workloads as wl;
    let v: serde_json::Value = serde_json::from_str(&wl::ollama_patch(wl::Gpu::Vulkan, Some(105))).unwrap();
    let c = &v["spec"]["template"]["spec"]["containers"][0];
    assert_eq!(c["name"], "ollama");
    assert_eq!(c["resources"]["limits"][wl::GPU_RESOURCE], 1);
    assert_eq!(c["env"][0]["name"], "OLLAMA_VULKAN");
    assert_eq!(v["spec"]["template"]["spec"]["securityContext"]["supplementalGroups"][0], 105);
    let n: serde_json::Value = serde_json::from_str(&wl::ollama_patch(wl::Gpu::Nvidia, None)).unwrap();
    assert_eq!(n["spec"]["template"]["spec"]["containers"][0]["resources"]["limits"]["nvidia.com/gpu"], 1);
    assert!(!wl::ollama_patch(wl::Gpu::Nvidia, None).contains("privileged"));
    let un = wl::ollama_unpatch();
    assert!(un.contains("OLLAMA_VULKAN") && un.contains("$patch") && un.contains("supplementalGroups"));
}

#[test]
fn where_ollama_runs_comes_from_the_marker_and_the_catalogue_record() {
    let dir = std::env::temp_dir().join(format!("iyakki-ollama-where-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(oc::where_runs(&dir, None), oc::Where::Nowhere);
    assert_eq!(oc::where_runs(&dir, Some("0.5.1".into())), oc::Where::Host("0.5.1".into()));
    // an older marker ("version 0.5.1") is understood as the host install
    mark_done(&dir, Item::Ollama, "version 0.5.1").unwrap();
    assert_eq!(oc::where_runs(&dir, Some("0.5.1".into())), oc::Where::Host("0.5.1".into()));
    mark_done(&dir, Item::Ollama, &oc::marker_detail(2, "")).unwrap();
    assert!(matches!(oc::where_runs(&dir, Some("0.5.1".into())), oc::Where::Cluster(d) if d.starts_with("in the cluster")));
    // presence: the host line says so; the cluster one needs the answer, else it is "installed earlier"
    let up = |_p: u16| Some("0.40.6".to_string());
    let down = |_p: u16| None;
    assert!(matches!(oc::presence(&dir, None, None, &up), Presence::Installed(d) if d.contains("in the cluster") && d.contains("0.40.6")));
    assert!(matches!(oc::presence(&dir, None, None, &down), Presence::Earlier(d) if d.contains("in the cluster")));
    let _ = std::fs::remove_file(marker(&dir, Item::Ollama));
    match oc::presence(&dir, Some("0.5.1".into()), None, &down) {
        Presence::Installed(d) => assert!(d.contains("Ollama runs on this machine's host") && !oc::is_cluster_detail(&d), "{d}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(oc::presence(&dir, None, None, &down), Presence::Missing);
    assert!(matches!(oc::presence(&dir, None, Some("/usr/bin/ollama".into()), &down), Presence::Elsewhere(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_move_is_decided_by_size_and_room_and_a_big_folder_is_offered_a_fresh_start() {
    assert_eq!(oc::decide(&host(5 * GB, Some(100 * GB))), oc::Decision::Copy);
    assert_eq!(oc::decide(&host(5 * GB, None)), oc::Decision::Copy, "unknown room does not block");
    assert_eq!(oc::decide(&host(21 * GB, Some(500 * GB))), oc::Decision::TooLarge, "past one step");
    assert_eq!(oc::decide(&host(5 * GB, Some(6 * GB))), oc::Decision::NoRoom, "a second copy needs room and a margin");
    assert_eq!(oc::decide(&host(0, Some(1 * GB))), oc::Decision::NoRoom);
    let big = oc::review_lines(oc::Mode::Move, &host(40 * GB, Some(500 * GB)), oc::Decision::TooLarge).join("\n");
    assert!(big.contains("too large") && big.contains("Start fresh in the cluster, keep host models where they are"), "{big}");
    let ok = oc::review_lines(oc::Mode::Move, &host(5 * GB, Some(100 * GB)), oc::Decision::Copy).join("\n");
    assert!(ok.contains("nothing is deleted from the host") && ok.contains("only then is it stopped") && ok.contains("5.0 GB") && ok.contains("There is room"), "{ok}");
    let fresh = oc::review_lines(oc::Mode::Fresh, &host(5 * GB, Some(100 * GB)), oc::Decision::Copy).join("\n");
    assert!(fresh.contains("not touched and not copied") && fresh.contains("starts empty"), "{fresh}");
    assert!(ok.contains("MIT") && ok.contains("Docker Hub"), "the licence is shown first");
}

#[test]
fn the_choice_is_left_for_the_job_once_and_nothing_else_is_taken_for_it() {
    let dir = std::env::temp_dir().join(format!("iyakki-ollama-mode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(oc::take_mode(&dir), None);
    oc::write_mode(&dir, oc::Mode::Move).unwrap();
    assert_eq!(oc::take_mode(&dir), Some(oc::Mode::Move));
    assert_eq!(oc::take_mode(&dir), None, "taken once");
    oc::write_mode(&dir, oc::Mode::Fresh).unwrap();
    assert_eq!(oc::take_mode(&dir), Some(oc::Mode::Fresh));
    std::fs::write(state_dir(&dir).join("ollama-mode"), "delete everything").unwrap();
    assert_eq!(oc::take_mode(&dir), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_plain_install_deploys_waits_for_the_answer_and_asks_the_shell_to_add_it_to_models() {
    let mut f = Fake::new();
    f.cluster_models = vec![];
    f.answers_after = 2;
    run_step(Item::Ollama, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["apply ollama", "rollout iyakki-ai/ollama"]);
    assert_eq!(f.afters, ["ollama"], "the host server is not stopped for a plain install");
    assert_eq!(f.marks.len(), 1);
    assert_eq!(f.marks[0].0, Item::Ollama);
    assert!(f.marks[0].1.starts_with("in the cluster") && f.marks[0].1.contains("127.0.0.1:11436"), "{}", f.marks[0].1);
    assert!(f.manifests[0].contains("kind: Deployment"));
}

#[test]
fn a_move_copies_after_the_agreement_checks_the_models_then_asks_to_stop_the_host() {
    let mut f = Fake::new();
    f.mode = Some(oc::Mode::Move);
    f.host = host(5 * GB, Some(100 * GB));
    f.cluster_models = vec!["llama3:8b".into(), "qwen2:7b".into()];
    f.host_models = Ok(vec!["llama3:8b".into()]);
    run_step(Item::Ollama, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["apply ollama", "rollout iyakki-ai/ollama", "copy /data/ollama/models"]);
    assert_eq!(f.afters, ["ollama", "ollama-host-stop"]);
    assert!(f.marks[0].1.contains("copied in") && f.marks[0].1.contains("2 model(s)"));
    assert!(f.lines.iter().any(|l| l.contains("the host folder is not changed")));
}

#[test]
fn a_move_whose_models_did_not_all_arrive_leaves_the_host_running_and_records_nothing() {
    let mut f = Fake::new();
    f.mode = Some(oc::Mode::Move);
    f.host = host(5 * GB, Some(100 * GB));
    f.cluster_models = vec!["qwen2:7b".into()];
    f.host_models = Ok(vec!["llama3:8b".into(), "qwen2:7b".into()]);
    let e = run_step(Item::Ollama, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap_err();
    assert!(e.contains("incomplete") && e.contains("llama3:8b") && e.contains("nothing was deleted"), "{e}");
    assert!(f.afters.is_empty() && f.marks.is_empty(), "the host is not stopped, nothing is marked");
}

#[test]
fn a_failed_copy_stops_the_job_before_anything_is_stopped_or_marked() {
    let mut f = Fake::new();
    f.mode = Some(oc::Mode::Move);
    f.host = host(1 * GB, Some(100 * GB));
    f.copy_fails = true;
    let e = run_step(Item::Ollama, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap_err();
    assert!(e.contains("not touched"), "{e}");
    assert!(f.afters.is_empty() && f.marks.is_empty());
}

#[test]
fn a_move_that_is_too_large_or_has_no_room_changes_nothing_at_all() {
    for h in [host(40 * GB, Some(500 * GB)), host(5 * GB, Some(5 * GB))] {
        let mut f = Fake::new();
        f.mode = Some(oc::Mode::Move);
        f.host = h;
        let e = run_step(Item::Ollama, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap_err();
        assert!(e.contains("nothing was changed") && e.contains("Start fresh"), "{e}");
        assert!(f.calls.is_empty() && f.afters.is_empty() && f.marks.is_empty(), "not even the manifest was applied");
    }
}

#[test]
fn starting_fresh_copies_nothing_keeps_the_host_models_and_stops_the_host_only_after_the_answer() {
    let mut f = Fake::new();
    f.mode = Some(oc::Mode::Fresh);
    f.host = host(40 * GB, Some(500 * GB));
    f.cluster_models = vec![];
    run_step(Item::Ollama, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert!(!f.calls.iter().any(|c| c.starts_with("copy")));
    assert_eq!(f.afters, ["ollama", "ollama-host-stop"]);
    assert!(f.marks[0].1.contains("started fresh") && f.marks[0].1.contains("kept"));
}

#[test]
fn a_cluster_ollama_that_never_answers_leaves_the_host_alone() {
    let mut f = Fake::new();
    f.mode = Some(oc::Mode::Fresh);
    f.answers_after = u32::MAX;
    let e = run_step(Item::Ollama, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap_err();
    assert!(e.contains("did not answer") && e.contains("host Ollama was left as it is"), "{e}");
    assert!(f.afters.is_empty() && f.marks.is_empty());
}

#[test]
fn the_status_line_says_where_ollama_runs() {
    assert!(oc::status_line(&oc::Where::Cluster("in the cluster: x".into())).contains("runs in the cluster"));
    assert!(oc::status_line(&oc::Where::Cluster("in the cluster: x".into())).contains("deployment ollama in iyakki-ai"));
    assert!(oc::status_line(&oc::Where::Host("0.5.1".into())).contains("runs on this machine's host"));
    assert!(oc::status_line(&oc::Where::Nowhere).contains("not installed"));
}

#[test]
fn the_tags_answer_is_read_for_the_model_names() {
    assert_eq!(crate::ollama::parse_tags(r#"{"models":[{"name":"llama3:8b","size":1},{"name":"qwen2:7b"}]}"#).unwrap(), ["llama3:8b", "qwen2:7b"]);
    assert_eq!(crate::ollama::parse_tags(r#"{"models":[]}"#).unwrap(), Vec::<String>::new());
    assert!(crate::ollama::parse_tags("<html>").is_err());
}
