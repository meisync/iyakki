//! Quick install with fakes only: no cluster, no network, no drive. The step runner gets a scripted [`StepEnv`]; the job launcher
//! starts a small shell script in place of `iyakki`.

use super::*;
use std::time::Instant;

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("iyakki-qi-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Fake {
    lines: Vec<String>,
    calls: Vec<String>,
    k3s: bool,
    elsewhere: Option<String>,
    /// The cluster becomes Ready after this many looks.
    ready_after: usize,
    looks: usize,
    install_fails: Option<String>,
    apply_fails: bool,
    /// Lemonade answers after this many looks.
    lem_after: usize,
    lem_looks: usize,
    slept: Duration,
    marks: Vec<(Item, String)>,
    registered: Vec<String>,
    /// What each prerequisite's own install says (default: ready).
    deps: Vec<(Item, DepState)>,
}

impl StepEnv for Fake {
    fn dep_state(&mut self, dep: Item) -> DepState {
        self.deps.iter().find(|d| d.0 == dep).map_or(DepState::Ready, |d| d.1.clone())
    }
    fn say(&mut self, line: &str) {
        self.lines.push(line.into());
    }
    fn k3s_present(&mut self) -> bool {
        self.k3s
    }
    fn k3s_elsewhere(&mut self) -> Option<String> {
        self.elsewhere.clone()
    }
    fn install_k3s(&mut self) -> Result<String, String> {
        self.calls.push("install_k3s".into());
        if let Some(e) = &self.install_fails {
            return Err(e.clone());
        }
        self.k3s = true;
        Ok("k3s v1.31.1+k3s1".into())
    }
    fn request_k3s_start(&mut self) -> Result<(), String> {
        self.calls.push("start".into());
        Ok(())
    }
    fn cluster_ready(&mut self) -> bool {
        self.looks += 1;
        self.looks > self.ready_after
    }
    fn install_cli(&mut self, id: &str) -> Result<String, String> {
        self.calls.push(format!("cli {id}"));
        Ok("v1.1.3".into())
    }
    fn register_dash_source(&mut self, cfg: crate::dash::connector::SourceConfig) -> Result<(), String> {
        self.registered.push(format!("{}|{}|{}", cfg.id, cfg.name, cfg.value("url")));
        Ok(())
    }
    fn kubectl_apply(&mut self, name: &str, manifest: &str) -> Result<String, String> {
        self.calls.push(if name == "elasticsearch" { "apply".into() } else { format!("apply {name}") });
        assert!(manifest.contains("kind: Deployment") && manifest.contains(if name == "lemonade" { LEMONADE_IMAGE } else { ES_IMAGE }));
        if self.apply_fails { Err("kubectl apply failed: forbidden".into()) } else { Ok("namespace/iyakki-data created\ndeployment.apps/elasticsearch created".into()) }
    }
    fn rollout(&mut self, ns: &str, name: &str, _t: Duration) -> Result<(), String> {
        self.calls.push(format!("rollout {ns}/{name}"));
        Ok(())
    }
    fn sleep(&mut self, d: Duration) {
        self.slept += d;
    }
    fn mark(&mut self, item: Item, detail: &str) -> Result<(), String> {
        self.marks.push((item, detail.into()));
        Ok(())
    }
    fn request_after(&mut self, what: &str) -> Result<(), String> {
        self.calls.push(format!("after {what}"));
        Ok(())
    }
    fn lemonade_ready(&mut self) -> bool {
        self.lem_looks += 1;
        self.lem_looks > self.lem_after
    }
    fn lemonade_pull(&mut self, model: &str) -> Result<(), String> {
        self.calls.push(format!("pull {model}"));
        Ok(())
    }
}

#[test]
fn the_order_puts_k3s_first_only_when_it_is_missing_and_the_target_always_last() {
    let none = |_: Item| false;
    assert_eq!(order(Item::K3s, &none), vec![Item::K3s]);
    assert_eq!(order(Item::Elasticsearch, &none), vec![Item::K3s, Item::Elasticsearch]);
    // DuckDB installs as a Python package: Python 3 (after k3s) comes before it, and only when it is missing
    assert_eq!(order(Item::DuckDb, &none), vec![Item::K3s, Item::Python, Item::DuckDb]);
    assert_eq!(order(Item::DuckDb, &|i| i == Item::Python), vec![Item::K3s, Item::DuckDb], "Python there: not again");
    let k3s_there = |i: Item| i == Item::K3s;
    assert_eq!(order(Item::Elasticsearch, &k3s_there), vec![Item::Elasticsearch], "k3s is there: not again");
    assert_eq!(order(Item::K3s, &k3s_there), vec![Item::K3s], "asking for k3s itself repairs it");
    let all = |_: Item| true;
    assert_eq!(order(Item::DuckDb, &all), vec![Item::DuckDb]);
    assert_eq!(order_text(&[Item::K3s, Item::Elasticsearch]), "1. k3s  \u{2192}  2. Elasticsearch");
    for i in Item::ALL {
        assert_eq!(Item::parse(i.id()), Some(i));
        assert!(!i.about().is_empty());
    }
    assert_eq!(Item::parse(" DuckDB "), Some(Item::DuckDb));
    assert_eq!(Item::parse("postgres"), None);
}

#[test]
fn labels_and_commands_name_the_step_and_run_this_program_on_the_data_folder() {
    let l = Launch { exe: "/usr/bin/iyakki".into(), data: "/var/lib/iyakki".into() };
    assert_eq!(label(Item::K3s, 1, 2), "Install k3s (1/2)");
    assert_eq!(label(Item::Elasticsearch, 2, 2), "Install Elasticsearch (2/2, after k3s)");
    assert_eq!(label(Item::DuckDb, 1, 1), "Install DuckDB (1/1)");
    let s = spec(Item::Elasticsearch, 2, 2, &l);
    assert_eq!(s.workspace, WORKSPACE);
    assert_eq!(s.cmd, Cmd::Argv(vec!["/usr/bin/iyakki".into(), "quick-install".into(), "elasticsearch".into(), "--data".into(), "/var/lib/iyakki".into()]));
    assert_eq!(s.cwd, PathBuf::from("/var/lib/iyakki"));
}

#[test]
fn k3s_is_installed_then_started_through_the_supervisor_and_marked() {
    let mut f = Fake::default();
    run_step(Item::K3s, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["install_k3s", "start"]);
    assert_eq!(f.marks, [(Item::K3s, "installed by Quick install".to_string())]);
    assert!(f.lines.iter().any(|l| l.starts_with("[1/2] installed: k3s v1.31.1")), "{:?}", f.lines);
    // the [n/m] lines are what the jobs engine shows as progress
    assert_eq!(crate::jobs::progress_hint(&f.lines.join("\n")), Some(100.0));
    // already installed: only the start request
    let mut f = Fake { k3s: true, ..Default::default() };
    run_step(Item::K3s, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["start"]);
}

#[test]
fn another_cluster_on_the_machine_is_used_as_it_is_and_nothing_is_installed() {
    let mut f = Fake { elsewhere: Some("/etc/rancher/k3s/k3s.yaml exists".into()), k3s: true, ..Default::default() };
    run_step(Item::K3s, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert!(f.calls.is_empty(), "{:?}", f.calls);
    assert!(f.lines[0].contains("already on this machine"));
}

#[test]
fn a_failed_k3s_install_fails_the_job_with_the_reason_and_marks_nothing() {
    let mut f = Fake { install_fails: Some("the download does not match the vendor's checksum".into()), ..Default::default() };
    let e = run_step(Item::K3s, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap_err();
    assert!(e.contains("checksum"), "{e}");
    assert!(f.marks.is_empty() && !f.calls.contains(&"start".to_string()));
}

#[test]
fn elasticsearch_waits_for_the_cluster_then_applies_waits_for_the_rollout_and_says_where_it_is() {
    let mut f = Fake { k3s: true, ready_after: 3, ..Default::default() };
    run_step(Item::Elasticsearch, &mut f, Duration::from_secs(600), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["apply", "rollout iyakki-data/elasticsearch"]);
    assert_eq!(f.slept, Duration::from_secs(15), "three looks of 5 s before Ready");
    assert!(f.lines.iter().any(|l| l.contains("waiting for k3s")), "{:?}", f.lines);
    assert!(f.lines.iter().any(|l| l.contains("deployment.apps/elasticsearch created")));
    assert_eq!(f.marks[0].0, Item::Elasticsearch);
    assert!(f.marks[0].1.contains("elasticsearch.iyakki-data.svc:9200"));
    assert_eq!(f.registered, ["elasticsearch-cluster|Cluster Elasticsearch|cluster://iyakki-data/elasticsearch:9200"], "a DASH source by the stable address, not a baked-in IP or localhost");
    assert!(f.marks[0].1.contains("cluster://iyakki-data/elasticsearch:9200"));
    assert_eq!(crate::jobs::progress_hint(&f.lines.join("\n")), Some(100.0));
}

#[test]
fn a_dependent_step_gives_up_when_the_cluster_never_comes_and_a_failed_apply_is_reported() {
    let mut f = Fake { k3s: false, ..Default::default() };
    let e = run_step(Item::Elasticsearch, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap_err();
    assert!(e.contains("not installed and Ready after 1 minutes"), "{e}");
    assert!(f.calls.is_empty(), "nothing applied without a cluster");
    let mut f = Fake { k3s: true, apply_fails: true, ..Default::default() };
    let e = run_step(Item::Elasticsearch, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap_err();
    assert!(e.contains("forbidden"), "{e}");
    assert!(f.marks.is_empty());
}

#[test]
fn duckdb_needs_the_cluster_then_installs_the_catalogue_program() {
    let mut f = Fake { k3s: true, ..Default::default() };
    run_step(Item::DuckDb, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["cli duckdb"]);
    assert_eq!(f.marks, [(Item::DuckDb, "version v1.1.3".to_string())]);
}

#[test]
fn the_manifest_is_namespaced_labelled_single_node_and_has_no_secret_values() {
    let m = elasticsearch_manifest();
    assert_eq!(m.matches("namespace: iyakki-data").count(), 3, "every namespaced object");
    assert!(m.contains("app.kubernetes.io/managed-by: iyakki"));
    assert!(m.contains("single-node") && m.contains("type: ClusterIP") && m.contains("storage: 5Gi"));
    assert!(!m.contains("NodePort") && !m.contains("LoadBalancer"), "nothing exposed outside the cluster");
    assert!(m.contains(&format!("image: {ES_IMAGE}")));
}

#[test]
fn markers_and_the_start_request_are_files_taken_once() {
    let t = Tmp::new("markers");
    assert_eq!(marked(&t.0, Item::Elasticsearch), None);
    mark_done(&t.0, Item::Elasticsearch, "http://x:9200").unwrap();
    assert_eq!(marked(&t.0, Item::Elasticsearch).as_deref(), Some("http://x:9200"));
    assert_eq!(take_start_request(&t.0), None);
    request_start(&t.0, "root").unwrap();
    assert_eq!(take_start_request(&t.0).as_deref(), Some("root"));
    assert_eq!(take_start_request(&t.0), None, "once");
}

/// A stand-in for `iyakki quick-install`: prints steps like the real one and ends.
fn fake_program(t: &Tmp) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = t.0.join("fake-iyakki");
    std::fs::write(&p, "#!/bin/sh\necho \"[1/2] fake $2\"\nsleep 0.3\necho \"[2/2] done $2\"\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[test]
fn starting_runs_one_detached_job_per_step_in_the_install_workspace_and_refuses_a_second_run() {
    let t = Tmp::new("start");
    let jobs = Jobs::new(&t.0.join("jobs"));
    let l = Launch { exe: fake_program(&t), data: t.0.join("data") };
    let none = |_: Item| false;
    let started = start(&jobs, "alice", Item::Elasticsearch, &none, &l).unwrap();
    assert_eq!(started.iter().map(|j| j.label.as_str()).collect::<Vec<_>>(), ["Install k3s (1/2)", "Install Elasticsearch (2/2, after k3s)"]);
    assert!(started.iter().all(|j| j.workspace == WORKSPACE));
    assert_eq!(started.iter().map(|j| item_of(j)).collect::<Vec<_>>(), [Some(Item::K3s), Some(Item::Elasticsearch)]);
    // while they run, the same install is refused
    let e = start(&jobs, "alice", Item::K3s, &none, &l).unwrap_err();
    assert!(e.contains("being installed already"), "{e}");
    assert_eq!(install_jobs(&jobs, "alice").len(), 2);
    assert!(jobs.visible("alice", "").is_empty(), "installer jobs are not in the chat's strip");
    let t0 = Instant::now();
    for j in &started {
        let done = jobs.wait("alice", &j.id, Duration::from_secs(20)).unwrap();
        assert_eq!(done.state, crate::jobs::State::Succeeded);
        assert!(done.last_line.starts_with("[2/2] done"), "{}", done.last_line);
    }
    assert!(t0.elapsed() < Duration::from_secs(20));
    // once they ended a new run is accepted
    // (DuckDB needs Python 3 too: with k3s and Python there, only DuckDB itself is a job)
    let again = start(&jobs, "alice", Item::DuckDb, &|i| i == Item::K3s || i == Item::Python, &l).unwrap();
    assert_eq!(again.len(), 1);
    jobs.wait("alice", &again[0].id, Duration::from_secs(20)).unwrap();
    let with_python = start(&jobs, "alice", Item::DuckDb, &|i| i == Item::K3s, &l).unwrap();
    assert_eq!(with_python.iter().map(|j| j.label.as_str()).collect::<Vec<_>>(), ["Install Python 3 (1/2)", "Install DuckDB (2/2, after k3s, Python 3)"]);
    for j in &with_python {
        jobs.wait("alice", &j.id, Duration::from_secs(20)).unwrap();
    }
}

#[test]
fn the_cli_refuses_an_unknown_item_without_touching_anything() {
    assert_eq!(cli_main(&["postgres".into()]), 2);
    assert_eq!(cli_main(&[]), 2);
}

#[test]
fn every_item_has_a_section_a_licence_and_the_pairs_say_pick_one_or_both() {
    for g in Group::ALL {
        assert!(!items_of(g).is_empty(), "{g:?}");
    }
    assert_eq!(items_of(Group::Voice), [Item::Kokoro, Item::VoicePack]);
    assert_eq!(items_of(Group::LocalModels), [Item::Lemonade, Item::Ollama, Item::MacHost, Item::GpuVulkan, Item::GpuNvidia]);
    assert_eq!(items_of(Group::AiCoding), [Item::ClaudeCode, Item::Copilot]);
    assert!(Group::Voice.note().contains("pick one or both") && Group::LocalModels.note().contains("pick one or both"));
    for i in Item::ALL {
        assert!(!i.licence().is_empty() && !i.about().is_empty(), "{i:?}");
        assert_eq!(Item::parse(i.id()), Some(i));
        if let Some(id) = i.cli_id() {
            assert!(crate::clis::def(id).is_some(), "{id} is in the CLI catalogue (reused, not duplicated)");
        }
    }
    assert!(!Item::MacHost.runs_as_job() && Item::MacHost.needs().is_empty() && Item::MacHost.licence().contains("nothing is installed by IYAKKI"), "the Mac model host row is a guide, not a job");
    assert!(Item::Kokoro.licence().contains("integration code only") && Item::Kokoro.licence().contains("own licence"));
    assert_eq!(order(Item::Kokoro, &|_| false), [Item::K3s, Item::Lemonade, Item::Kokoro]);
    // the runtime an npm tool installs with comes first, like k3s before DuckDB (when the catalogue says the tool is an npm one)
    let npm = matches!(crate::clis::def("github-copilot").map(|d| d.source), Some(crate::clis::Source::Npm(_)));
    assert_eq!(order(Item::Copilot, &|_| false), if npm { vec![Item::Node, Item::Copilot] } else { vec![Item::Copilot] });
    assert_eq!(order(Item::Copilot, &|i| i == Item::Node), [Item::Copilot], "Node.js there: not again");
    assert_eq!(order_text(&order(Item::DuckDb, &|_| false)), "1. k3s  \u{2192}  2. Python 3  \u{2192}  3. DuckDB");
    assert_eq!(order(Item::Lemonade, &|i| i == Item::K3s), [Item::Lemonade]);
    assert_eq!(Item::Lemonade.cli_id(), None, "Lemonade is a Deployment in k3s, not a catalogue program");
}

#[test]
fn coding_agents_install_from_the_catalogue_and_servers_ask_to_be_started_and_connected() {
    let mut f = Fake::default();
    run_step(Item::ClaudeCode, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["cli claude-code"]);
    assert!(f.lines.last().unwrap().contains("claude auth login"));
    let mut f = Fake::default();
    run_step(Item::Ollama, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["cli ollama", "after ollama"]);
    // the media player: the catalogue's static ffmpeg, no cluster, no runtime first, in its own section
    let mut f = Fake::default();
    run_step(Item::Ffmpeg, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["cli ffmpeg"]);
    assert_eq!(items_of(Group::Media), [Item::Ffmpeg]);
    assert_eq!((Group::Media.title(), Item::Ffmpeg.title()), ("Media", "Media player (ffmpeg)"));
    assert!(Item::Ffmpeg.about().contains("GPLv3") && Item::Ffmpeg.about().contains("checksum"));
    assert_eq!(order(Item::Ffmpeg, &|_| false), [Item::Ffmpeg]);
    assert_eq!(action_label(Item::Ffmpeg, &Presence::Missing, None), "Install Media player (ffmpeg)");
    assert!(action_label(Item::Ffmpeg, &Presence::Installed("version latest".into()), None).starts_with("Update "));
    let mut f = Fake::default();
    run_step(Item::Lemonade, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap_err();
    assert!(f.calls.is_empty(), "without a cluster nothing is applied");
    let mut f = Fake { k3s: true, ..Default::default() };
    run_step(Item::Lemonade, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["apply lemonade", "rollout iyakki-ai/lemonade", "after lemonade"]);
    assert_eq!(f.marks.len(), 1);
    assert_eq!(crate::jobs::progress_hint(&f.lines.join("\n")), Some(100.0));
}

#[test]
fn the_lemonade_manifest_is_loopback_only_runs_as_the_images_user_and_has_a_pinned_image() {
    let m = lemonade_manifest();
    assert!(m.contains(LEMONADE_IMAGE) && !LEMONADE_IMAGE.ends_with(":latest"));
    assert!(m.contains("hostIP: 127.0.0.1") && m.contains("hostPort: 13305"), "the API has no login: published on loopback only");
    assert!(m.contains("chown -R 10001:10001 /data") && m.contains("claimName: lemonade-data"));
    for sub in ["huggingface", "llama", "cache", "config"] {
        assert!(m.contains(&format!("subPath: {sub}")), "{sub}");
    }
    assert!(m.contains("app.kubernetes.io/managed-by: iyakki") && m.contains(&format!("namespace: {LEMONADE_NAMESPACE}")));
    assert!(!m.contains("privileged") && !m.to_ascii_lowercase().contains("password"));
}

#[test]
fn kokoro_deploys_lemonade_itself_when_it_is_not_there_waits_for_it_then_pulls_the_model() {
    // Lemonade answers already: nothing is applied
    let mut f = Fake::default();
    run_step(Item::Kokoro, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["pull kokoro-v1", "after kokoro"], "after the pull the shell is asked to make it the reading voice");
    // not there yet: the cluster is checked, Lemonade deployed and connected, then the model pulled
    let mut f = Fake { k3s: true, lem_after: 1, ..Default::default() };
    run_step(Item::Kokoro, &mut f, Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["apply lemonade", "rollout iyakki-ai/lemonade", "after lemonade", "pull kokoro-v1", "after kokoro"]);
    // still not answering after the deploy: the wait ends with the port in the message
    let mut f = Fake { k3s: true, lem_after: usize::MAX, ..Default::default() };
    assert!(run_step(Item::Kokoro, &mut f, Duration::from_secs(20), Duration::from_secs(5)).unwrap_err().contains("did not answer"));
    assert!(!f.calls.contains(&"pull kokoro-v1".to_string()));
    // no cluster: stops with the cluster's reason instead of waiting for a server that cannot come
    let mut f = Fake { lem_after: usize::MAX, ..Default::default() };
    assert!(run_step(Item::Kokoro, &mut f, Duration::from_secs(20), Duration::from_secs(5)).unwrap_err().contains("k3s was not installed"));
    assert!(f.calls.is_empty());
}

#[test]
fn server_requests_are_files_taken_once_per_server() {
    let t = Tmp::new("after");
    request_after(&t.0, "ollama", "root").unwrap();
    assert_eq!(take_after(&t.0, "lemonade"), None);
    assert_eq!(take_after(&t.0, "ollama").as_deref(), Some("root"));
    assert_eq!(take_after(&t.0, "ollama"), None);
}


#[test]
fn a_step_whose_prerequisite_failed_first_stops_at_once_with_a_plain_message_and_never_waits() {
    // Kokoro needs Lemonade; Lemonade's own job failed: no waiting, no deploy
    let mut f = Fake { deps: vec![(Item::Lemonade, DepState::Failed("error: kubectl apply failed: forbidden".into()))], ..Default::default() };
    let e = run_step(Item::Kokoro, &mut f, Duration::from_secs(25 * 60), Duration::from_secs(5)).unwrap_err();
    assert!(e.starts_with("Lemonade failed first") && e.contains("See its log") && e.contains("forbidden"), "{e}");
    assert_eq!(f.slept, Duration::ZERO, "it did not wait out the timeout");
    assert!(f.calls.is_empty(), "{:?}", f.calls);
    // Elasticsearch needs k3s whose job failed
    let mut f = Fake { deps: vec![(Item::K3s, DepState::Failed("the download does not match the vendor's checksum".into()))], ..Default::default() };
    let e = run_step(Item::Elasticsearch, &mut f, Duration::from_secs(25 * 60), Duration::from_secs(5)).unwrap_err();
    assert!(e.starts_with("k3s failed first") && e.contains("checksum"), "{e}");
    assert_eq!(f.slept, Duration::ZERO);
    // the dependency fails while a step is already waiting: it stops at the next look, not after the timeout
    struct Late(Fake, usize);
    impl StepEnv for Late {
        fn dep_state(&mut self, dep: Item) -> DepState {
            self.1 += 1;
            if self.1 > 3 { DepState::Failed("it broke".into()) } else { self.0.dep_state(dep) }
        }
        fn say(&mut self, l: &str) { self.0.say(l) }
        fn k3s_present(&mut self) -> bool { self.0.k3s_present() }
        fn k3s_elsewhere(&mut self) -> Option<String> { self.0.k3s_elsewhere() }
        fn install_k3s(&mut self) -> Result<String, String> { self.0.install_k3s() }
        fn request_k3s_start(&mut self) -> Result<(), String> { self.0.request_k3s_start() }
        fn cluster_ready(&mut self) -> bool { self.0.cluster_ready() }
        fn install_cli(&mut self, id: &str) -> Result<String, String> { self.0.install_cli(id) }
        fn kubectl_apply(&mut self, n: &str, m: &str) -> Result<String, String> { self.0.kubectl_apply(n, m) }
        fn rollout(&mut self, a: &str, b: &str, t: Duration) -> Result<(), String> { self.0.rollout(a, b, t) }
        fn sleep(&mut self, d: Duration) { self.0.sleep(d) }
        fn mark(&mut self, i: Item, d: &str) -> Result<(), String> { self.0.mark(i, d) }
        fn request_after(&mut self, w: &str) -> Result<(), String> { self.0.request_after(w) }
        fn lemonade_ready(&mut self) -> bool { self.0.lemonade_ready() }
        fn lemonade_pull(&mut self, m: &str) -> Result<(), String> { self.0.lemonade_pull(m) }
    }
    let mut l = Late(Fake { k3s: true, ready_after: usize::MAX, ..Default::default() }, 0);
    let e = run_step(Item::DuckDb, &mut l, Duration::from_secs(25 * 60), Duration::from_secs(5)).unwrap_err();
    assert!(e.contains("k3s failed first") && e.contains("it broke"), "{e}");
    assert!(l.0.slept <= Duration::from_secs(30), "stopped within a few looks, not 25 minutes: {:?}", l.0.slept);
}

#[test]
fn a_runtime_a_tool_needs_is_waited_for_and_its_failure_stops_the_tool() {
    let npm = matches!(crate::clis::def("github-copilot").map(|d| d.source), Some(crate::clis::Source::Npm(_)));
    if !npm {
        return;
    }
    let mut f = Fake { deps: vec![(Item::Node, DepState::Failed("Node.js was not found".into()))], ..Default::default() };
    let e = run_step(Item::Copilot, &mut f, Duration::from_secs(600), Duration::from_secs(5)).unwrap_err();
    assert!(e.starts_with("Node.js failed first"), "{e}");
    assert!(f.calls.is_empty(), "the tool was not installed without its runtime");
    let mut f = Fake { deps: vec![(Item::Node, DepState::Ready)], ..Default::default() };
    run_step(Item::Copilot, &mut f, Duration::from_secs(600), Duration::from_secs(5)).unwrap();
    assert_eq!(f.calls, ["cli github-copilot"]);
    // pending forever: gives up after the wait, saying so
    let mut f = Fake { deps: vec![(Item::Node, DepState::Pending)], ..Default::default() };
    let e = run_step(Item::Copilot, &mut f, Duration::from_secs(20), Duration::from_secs(5)).unwrap_err();
    assert!(e.contains("Node.js was not installed after"), "{e}");
}

fn job_of(item: &str, state: crate::jobs::State, started: u64, last: &str) -> Job {
    Job { id: format!("j{started}"), label: format!("Install {item}"), command: format!("iyakki quick-install {item} --data /d"), cwd: "/".into(), owner: "root".into(), workspace: WORKSPACE.into(), pid: 1, state, started_ms: started, ended_ms: 0, progress: None, last_line: last.into(), dismissed: false }
}

#[test]
fn the_state_of_a_prerequisite_comes_from_the_machine_then_its_newest_job() {
    use crate::jobs::State;
    let jobs = vec![job_of("lemonade", State::Failed(1), 10, "error: no pod"), job_of("lemonade", State::Running, 20, ""), job_of("k3s", State::Failed(1), 5, "error: x")];
    assert_eq!(dep_state_from(&jobs, Item::Lemonade, false), DepState::Pending, "a retry is running: the newest job counts");
    assert_eq!(dep_state_from(&jobs, Item::K3s, false), DepState::Failed("error: x".into()));
    assert_eq!(dep_state_from(&jobs, Item::K3s, true), DepState::Ready, "it is on the machine after all");
    assert_eq!(dep_state_from(&[], Item::Node, false), DepState::Pending);
    assert_eq!(dep_state_from(&[job_of("nodejs", State::Succeeded, 1, "done")], Item::Node, false), DepState::Ready);
    assert_eq!(dep_state_from(&[job_of("nodejs", State::Lost, 1, "")], Item::Node, false), DepState::Failed("lost".into()));
    // Retry is offered on what failed last
    let mut all = jobs.clone();
    assert_eq!(failed_items(&all), [Item::K3s]);
    all.push(job_of("elasticsearch", State::Stopped, 30, ""));
    assert_eq!(failed_items(&all), [Item::Elasticsearch, Item::K3s]);
    all.push(job_of("k3s", State::Succeeded, 40, "done"));
    assert_eq!(failed_items(&all), [Item::Elasticsearch], "a later success clears it");
}

#[test]
fn upgrades_are_offered_only_when_the_vendor_publishes_a_newer_version() {
    assert!(same_version("v1.31.1+k3s1", "v1.31.1-k3s1") && same_version("1.2.3", "v1.2.3") && !same_version("1.2.3", "1.2.4") && !same_version("", ""));
    let inst = vec![(Item::K3s, Some("v1.31.1+k3s1".to_string())), (Item::DuckDb, Some("v1.1.3".into())), (Item::ClaudeCode, Some("2.0.1".into())), (Item::Elasticsearch, None), (Item::Node, None)];
    let fetch = |id: &str| -> Result<String, String> {
        match id {
            "k3s" => Ok("v1.32.0+k3s1".into()),
            "duckdb" => Ok("v1.1.3".into()),
            _ => Err("publishes only \"latest\"".into()),
        }
    };
    let r = upgrades_with(&inst, &fetch);
    assert_eq!(r[0], (Item::K3s, Latest::Newer("v1.32.0+k3s1".into())));
    assert_eq!(r[1], (Item::DuckDb, Latest::Current("v1.1.3".into())));
    assert!(matches!(&r[2].1, Latest::Unknown(_)));
    assert_eq!((r[3].1.clone(), r[4].1.clone()), (Latest::NotApplicable, Latest::NotApplicable));
    let inst_p = Presence::Installed("version v1.31.1+k3s1".into());
    assert_eq!(action_label(Item::K3s, &inst_p, Some(&r[0].1)), "Upgrade k3s to 1.32.0+k3s1");
    assert_eq!(action_label(Item::DuckDb, &inst_p, Some(&r[1].1)), "Reinstall DuckDB");
    assert_eq!(action_label(Item::ClaudeCode, &inst_p, Some(&r[2].1)), "Update Claude Code");
    assert_eq!(action_label(Item::Elasticsearch, &inst_p, None), "Reinstall Elasticsearch");
    assert_eq!(action_label(Item::Node, &Presence::Missing, None), "Install Node.js");
    assert!(state_text(&inst_p, Some(&r[0].1)).ends_with("upgrade available: 1.32.0+k3s1"));
    assert!(state_text(&inst_p, Some(&r[1].1)).ends_with("up to date"));
    // starting an install of something that is installed is an Upgrade job
    assert_eq!(label_with("Upgrade", Item::K3s, 1, 1), "Upgrade k3s (1/1)");
}

fn services_here(t: &Tmp) -> std::sync::Arc<crate::services::Services> {
    struct Never;
    impl iyakki_proxy::Upstream for Never {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("no network in tests".into())
        }
    }
    let up: std::sync::Arc<dyn iyakki_proxy::Upstream> = std::sync::Arc::new(Never);
    let svc = crate::services::Services::open(&t.0.join("svc"), up.clone(), up).unwrap();
    svc.clis.set_detect_present(true);
    svc
}

fn put_tool(svc: &crate::services::Services, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let d = svc.clis.bin_dir();
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(name), script).unwrap();
    std::fs::set_permissions(d.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn tools_put_in_the_tools_folder_by_hand_show_as_present_with_their_version_and_are_not_replaced_by_default() {
    let t = Tmp::new("byhand");
    let svc = services_here(&t);
    put_tool(&svc, "ffmpeg", "#!/bin/sh\necho 'ffmpeg version 7.0.2-static'\n");
    put_tool(&svc, "node", "#!/bin/sh\necho v26.12.0\n");
    let p = presence(&svc, Item::Ffmpeg);
    assert!(matches!(&p, Presence::Elsewhere(d) if d.contains("7.0.2")), "{p:?}");
    assert!(p.present(), "it counts as there: nothing is installed first for it");
    let n = presence(&svc, Item::Node);
    assert!(matches!(&n, Presence::Elsewhere(d) if d.contains("26.12.0")), "{n:?}");
    // the button does not offer a replacement: it is its own, deliberate choice
    assert_eq!(action_label(Item::Ffmpeg, &p, None), "Install Media player (ffmpeg) (own copy)");
    // a tool that is not there stays Missing (Elasticsearch has no program to find)
    assert_eq!(presence(&svc, Item::Elasticsearch), Presence::Missing);
    // the facts gather without failing and name no problem for working tools
    let f = gather_facts(&svc);
    assert!(f.unhealthy.is_empty(), "{:?}", f.unhealthy);
    assert!(f.presence.iter().any(|(i, p)| *i == Item::Node && p.present()));
}

#[test]
fn the_order_line_names_the_runtimes_the_catalogue_installs_underneath() {
    let before = |i: Item| -> Vec<&'static str> { if i == Item::DuckDb { vec!["uv", "python"] } else { vec![] } };
    let steps = [Item::K3s, Item::Python, Item::DuckDb];
    let titles = order_titles_with(&steps, &|i| if i == Item::Python { vec!["uv"] } else { before(i) });
    assert_eq!(titles, ["k3s", "uv", "Python 3", "DuckDB"], "uv then Python, once each, before DuckDB");
    assert_eq!(titles_text(&titles), "1. k3s  \u{2192}  2. uv  \u{2192}  3. Python 3  \u{2192}  4. DuckDB");
    assert_eq!(order_titles_with(&[Item::Node, Item::ClaudeCode], &|_| vec![]), ["Node.js", "Claude Code"]);
}

#[test]
fn the_mac_firmware_step_is_its_own_section_runs_as_a_job_with_the_source_on_the_command_line_and_marks_itself_done() {
    assert_eq!(Item::parse("mac-firmware"), Some(Item::MacFirmware));
    assert_eq!(Item::MacFirmware.group(), Group::Mac);
    assert_eq!(items_of(Group::Mac), [Item::MacFirmware, Item::MacCamera]);
    assert!(Item::MacFirmware.title().contains("Mac Wi-Fi and Bluetooth firmware") && Item::MacFirmware.title().contains("from your Mac"));
    assert!(Item::MacFirmware.licence().contains("never ships") && Item::MacFirmware.about().contains("never uploaded"));
    assert!(Item::MacFirmware.runs_as_job() && Item::MacFirmware.needs_now().is_empty() && Item::MacFirmware.cli_id().is_none());
    let l = Launch { exe: "/usr/bin/iyakki".into(), data: "/var/lib/iyakki".into() };
    // the quick-install path starts it with Apple's download as the source (the default of the step)
    assert_eq!(spec(Item::MacFirmware, 1, 1, &l).cmd, Cmd::Argv(vec!["/usr/bin/iyakki".into(), "quick-install".into(), "mac-firmware".into(), "--data".into(), "/var/lib/iyakki".into()]));
    // the Mac drivers page starts it with the source the person chose
    let s = mac_firmware_spec(&l, &crate::macfw::Source::Folder("/mnt/usb/iyakki-wifi-firmware".into()));
    assert_eq!(s.workspace, WORKSPACE);
    assert_eq!(s.cmd, Cmd::Argv(["/usr/bin/iyakki", "quick-install", "mac-firmware", "--data", "/var/lib/iyakki", "--source", "folder", "--path", "/mnt/usb/iyakki-wifi-firmware"].map(String::from).to_vec()));
    // a step that finishes leaves its result line as the marker; a failure is the job's failure
    struct MacOk(bool);
    impl StepEnv for MacOk {
        fn dep_state(&mut self, _: Item) -> DepState { DepState::Ready }
        fn say(&mut self, _: &str) {}
        fn k3s_present(&mut self) -> bool { false }
        fn k3s_elsewhere(&mut self) -> Option<String> { None }
        fn install_k3s(&mut self) -> Result<String, String> { Err("no".into()) }
        fn request_k3s_start(&mut self) -> Result<(), String> { Ok(()) }
        fn cluster_ready(&mut self) -> bool { false }
        fn install_cli(&mut self, _: &str) -> Result<String, String> { Err("no".into()) }
        fn kubectl_apply(&mut self, _: &str, _: &str) -> Result<String, String> { Err("no".into()) }
        fn rollout(&mut self, _: &str, _: &str, _: Duration) -> Result<(), String> { Err("no".into()) }
        fn sleep(&mut self, _: Duration) {}
        fn mark(&mut self, item: Item, detail: &str) -> Result<(), String> {
            assert_eq!(item, Item::MacFirmware);
            assert!(detail.contains("Wi-Fi: wlan0 appeared"));
            self.0 = true;
            Ok(())
        }
        fn request_after(&mut self, _: &str) -> Result<(), String> { Ok(()) }
        fn lemonade_ready(&mut self) -> bool { false }
        fn lemonade_pull(&mut self, _: &str) -> Result<(), String> { Err("no".into()) }
        fn mac_firmware(&mut self) -> Result<String, String> { Ok("Wi-Fi: wlan0 appeared".into()) }
    }
    let mut env = MacOk(false);
    run_step(Item::MacFirmware, &mut env, Duration::ZERO, Duration::ZERO).unwrap();
    assert!(env.0, "marked done");
    // a step environment that cannot do it says so plainly
    let mut f = Fake::default();
    assert!(run_step(Item::MacFirmware, &mut f, Duration::ZERO, Duration::ZERO).unwrap_err().contains("not available here"));
    assert!(f.marks.is_empty());
    // a bad source is a usage error, not a download
    assert_eq!(cli_main(&["mac-firmware".into(), "--source".into(), "image".into()]), 2);
}

fn lem_model(id: &str, kind: crate::lemonade::ModelType, downloaded: bool) -> crate::lemonade::Model {
    crate::lemonade::Model { id: id.into(), kind, recipe: String::new(), checkpoint: String::new(), labels: vec![], downloaded, loaded: false, suggested: false, size_gb: None, image_defaults: None }
}

#[test]
fn lemonade_and_kokoro_status_comes_from_the_server_and_the_marker_is_only_a_fallback() {
    use crate::lemonade::ModelType;
    let t = Tmp::new("live-status");
    let svc = services_here(&t);
    let none = || None;
    let with_kokoro = || Some(vec![lem_model("Qwen3-0.6B-GGUF", ModelType::Chat, true), lem_model("kokoro-v1", ModelType::TextToSpeech, true)]);
    let without_kokoro = || Some(vec![lem_model("Qwen3-0.6B-GGUF", ModelType::Chat, true), lem_model("kokoro-v1", ModelType::TextToSpeech, false)]);
    // no marker, nothing answers: not installed
    assert_eq!(presence_with(&svc, Item::Kokoro, &none), Presence::Missing);
    mark_done(svc.dir(), Item::Kokoro, "kokoro-v1 in Lemonade").unwrap();
    mark_done(svc.dir(), Item::Lemonade, "lemonade in k3s").unwrap();
    // marker present but Lemonade does not answer (cluster down or reset): installed earlier, not running
    for it in [Item::Kokoro, Item::Lemonade] {
        let p = presence_with(&svc, it, &none);
        assert!(matches!(p, Presence::Earlier(_)), "{it:?}: {p:?}");
        assert!(p.word().starts_with("installed earlier, not running"), "{}", p.word());
    }
    // answering but kokoro missing (or listed, not downloaded): not installed, even with the marker
    assert_eq!(presence_with(&svc, Item::Kokoro, &without_kokoro), Presence::Missing);
    assert_eq!(presence_with(&svc, Item::Kokoro, &|| Some(vec![])), Presence::Missing);
    // everything present: installed
    assert!(matches!(presence_with(&svc, Item::Kokoro, &with_kokoro), Presence::Installed(_)));
    assert!(matches!(presence_with(&svc, Item::Lemonade, &with_kokoro), Presence::Installed(d) if d.contains("13305")));
    // answering with no marker at all still counts: reality wins over the file
    std::fs::remove_dir_all(state_dir(svc.dir())).unwrap();
    assert!(matches!(presence_with(&svc, Item::Kokoro, &with_kokoro), Presence::Installed(_)));
    assert_eq!(presence_with(&svc, Item::Lemonade, &none), Presence::Missing);
}

#[test]
fn the_camera_firmware_job_is_an_item_with_its_own_command_and_is_recognised_while_it_runs() {
    assert_eq!(Item::parse("mac-camera"), Some(Item::MacCamera), "the Setup tab lists it; the CLI word is the same");
    assert!(!Item::MacCamera.runs_as_job() && Item::MacCamera.is_hardware() && Item::MacCamera.group() == Group::Mac, "started by start_camera_firmware, not by the installer's order");
    assert_eq!(Item::MacCamera.id(), CAMERA_JOB);
    let l = Launch { exe: "/usr/bin/iyakki".into(), data: "/var/lib/iyakki".into() };
    let s = camera_firmware_spec(&l);
    assert_eq!(s.workspace, WORKSPACE);
    assert_eq!(s.cmd, Cmd::Argv(["/usr/bin/iyakki", "quick-install", "mac-camera", "--data", "/var/lib/iyakki"].map(String::from).to_vec()));
    let mut j = job_of("mac-camera", crate::jobs::State::Running, 1, "");
    assert!(is_camera_job(&j));
    j.command = "iyakki quick-install mac-firmware --data /d".into();
    assert!(!is_camera_job(&j));
}

#[test]
fn the_firmware_packs_are_hardware_items_with_their_own_job_licence_and_cli_word() {
    let packs = [Item::FwQualcommBt, Item::FwRealtekNet, Item::FwBroadcomNet, Item::FwAtheros9170, Item::FwMediatek];
    let mut with_hw = packs.to_vec();
    with_hw.push(Item::FwHardware);
    assert_eq!(items_of(Group::Firmware), with_hw, "the five packs and the Hardware firmware item");
    let h = Item::FwHardware;
    assert_eq!(Item::parse(h.id()), Some(h));
    assert!(h.fw_def().is_none() && h.is_hardware() && !h.runs_as_job() && h.group() == Group::Firmware && h.needs().is_empty());
    assert!(h.licence().contains("never shipped") && h.licence().contains("Various per file"));
    let hs = hardware_firmware_spec(&Launch { exe: "/usr/bin/iyakki".into(), data: "/var/lib/iyakki".into() }, &HwFirmwareOpts { from: Some("/media/usb".into()), with: vec!["nvidia".into()], link: None });
    assert_eq!(hs.cmd, Cmd::Argv(["/usr/bin/iyakki", "quick-install", "fw-hardware", "--data", "/var/lib/iyakki", "--from", "/media/usb", "--with", "nvidia"].map(String::from).to_vec()));
    assert_eq!(Group::Firmware.title(), "Hardware firmware");
    for i in packs {
        let d = i.fw_def().expect("a pack");
        assert_eq!(Item::parse(i.id()), Some(i), "`iyakki quick-install {}` is the same word", i.id());
        assert_eq!(i.id(), d.pack_id);
        assert!(i.is_hardware() && !i.runs_as_job() && i.group() == Group::Firmware && i.needs().is_empty() && i.cli_id().is_none(), "{i:?}: its own job, only where the hardware is");
        assert!(i.licence().contains(d.licence_note) && i.about().contains("Not in the images") && i.about().contains("folder or USB stick") && !i.title().is_empty());
        assert!(i.download_mb() >= 1);
    }
    assert!(Item::FwRealtekNet.licence().contains("hexadecimal or equivalent format") && Item::FwQualcommBt.licence().contains("third-party licence terms") && Item::FwAtheros9170.licence().contains("GPL"));
    assert!(Item::Node.fw_def().is_none());
    // the job: this program, `quick-install fw-x --data D`, with the folder for the offline route
    let l = Launch { exe: "/usr/bin/iyakki".into(), data: "/var/lib/iyakki".into() };
    let s = firmware_pack_spec(&l, Item::FwRealtekNet, None);
    assert_eq!(s.workspace, WORKSPACE);
    assert_eq!(s.cmd, Cmd::Argv(["/usr/bin/iyakki", "quick-install", "fw-realtek-net", "--data", "/var/lib/iyakki"].map(String::from).to_vec()));
    let s = firmware_pack_spec(&l, Item::FwRealtekNet, Some(Path::new("/media/usb/fw")));
    assert_eq!(s.cmd, Cmd::Argv(["/usr/bin/iyakki", "quick-install", "fw-realtek-net", "--data", "/var/lib/iyakki", "--from", "/media/usb/fw"].map(String::from).to_vec()));
    assert!(s.label.contains("from a folder"));
    let j = job_of("fw-realtek-net", crate::jobs::State::Running, 1, "");
    assert_eq!(item_of(&j), Some(Item::FwRealtekNet));
}

#[test]
fn a_firmware_pack_job_starts_only_after_the_licence_was_accepted_and_one_at_a_time() {
    let t = Tmp::new("fwstart");
    let jobs = Jobs::new(&t.0.join("jobs"));
    let l = Launch { exe: fake_program(&t), data: t.0.join("data") };
    let e = start_firmware_pack(&jobs, "root", Item::FwBroadcomNet, None, &l).unwrap_err();
    assert!(e.contains("accept the vendor's licence first") && install_jobs(&jobs, "root").is_empty(), "{e}");
    assert!(start_firmware_pack(&jobs, "root", Item::Node, None, &l).unwrap_err().contains("not a firmware pack"));
    crate::fwpack::accept(&l.data, Item::FwBroadcomNet.fw_def().unwrap(), "admin", 1).unwrap();
    // an acceptance of another pack does not count
    assert!(start_firmware_pack(&jobs, "root", Item::FwRealtekNet, None, &l).is_err());
    let j = start_firmware_pack(&jobs, "root", Item::FwBroadcomNet, None, &l).unwrap();
    assert_eq!(item_of(&j), Some(Item::FwBroadcomNet));
    assert!(start_firmware_pack(&jobs, "root", Item::FwBroadcomNet, None, &l).unwrap_err().contains("being installed already"));
    jobs.wait("root", &j.id, Duration::from_secs(20)).unwrap();
}

#[test]
fn a_firmware_pack_item_reads_as_installed_from_its_driver_pack_record() {
    let t = Tmp::new("fwpresence");
    let svc = services_here(&t);
    assert_eq!(presence(&svc, Item::FwRealtekNet), Presence::Missing);
    let d = Item::FwRealtekNet.fw_def().unwrap();
    let (fs, _) = iyakki_drivers::apply::Fs::for_data(svc.dir());
    let p = crate::fwpack::build_pack(crate::fwpack::table(), d, &[("rtl_nic/rtl8168h-2.fw".into(), vec![1, 2, 3])], &[("LICENSE.r8169".into(), vec![4])]);
    iyakki_drivers::apply::install_pack(&fs, &iyakki_drivers::run::Dry, d.pack_id, "linux-firmware 1a935bfb33", d.licence_id, "test", "local", &p).unwrap();
    match presence(&svc, Item::FwRealtekNet) {
        Presence::Installed(w) => assert!(w.starts_with("1 files"), "{w}: the licence text is not counted as a firmware file"),
        other => panic!("{other:?}"),
    }
    assert_eq!(presence(&svc, Item::FwQualcommBt), Presence::Missing);
}
