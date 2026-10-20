//! The "Local image registry" Quick install entry, with fakes only.

use super::*;

#[derive(Default)]
struct Env {
    lines: Vec<String>,
    marks: Vec<String>,
    other: bool,
    ready: bool,
    created: u32,
    fail: Option<String>,
}

impl StepEnv for Env {
    fn say(&mut self, l: &str) {
        self.lines.push(l.into());
    }
    fn dep_state(&mut self, _: Item) -> DepState {
        DepState::Ready
    }
    fn k3s_present(&mut self) -> bool {
        self.ready
    }
    fn k3s_elsewhere(&mut self) -> Option<String> {
        None
    }
    fn install_k3s(&mut self) -> Result<String, String> {
        Err("no".into())
    }
    fn request_k3s_start(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn cluster_ready(&mut self) -> bool {
        self.ready
    }
    fn install_cli(&mut self, _: &str) -> Result<String, String> {
        Err("no".into())
    }
    fn kubectl_apply(&mut self, _: &str, _: &str) -> Result<String, String> {
        Err("no".into())
    }
    fn rollout(&mut self, _: &str, _: &str, _: Duration) -> Result<(), String> {
        Ok(())
    }
    fn sleep(&mut self, _: Duration) {}
    fn mark(&mut self, i: Item, d: &str) -> Result<(), String> {
        self.marks.push(format!("{} {d}", i.id()));
        Ok(())
    }
    fn request_after(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn lemonade_ready(&mut self) -> bool {
        false
    }
    fn lemonade_pull(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn registry_on_other_cluster(&mut self) -> bool {
        self.other
    }
    fn create_registry(&mut self) -> Result<String, String> {
        self.created += 1;
        self.fail.clone().map_or(Ok("localhost:30500".into()), Err)
    }
}

#[test]
fn the_entry_is_in_the_cluster_section_and_needs_k3s() {
    assert_eq!(Item::Registry.group(), Group::Cluster);
    assert_eq!(Item::parse("registry"), Some(Item::Registry));
    assert_eq!(order(Item::Registry, &|_| false), [Item::K3s, Item::Registry]);
    assert_eq!(order(Item::Registry, &|i| i == Item::K3s), [Item::Registry]);
    assert!(Item::Registry.licence().contains("Apache-2.0") && Item::Registry.licence().contains("does not ship"));
    assert!(Item::Registry.runs_as_job());
}

#[test]
fn the_step_waits_for_a_ready_cluster_then_creates_and_marks() {
    let mut e = Env { ready: true, ..Default::default() };
    run_step(Item::Registry, &mut e, Duration::from_secs(10), Duration::from_secs(1)).unwrap();
    assert_eq!((e.created, e.marks.clone()), (1, vec!["registry pods pull localhost:30500".to_string()]));
    // the cluster never gets Ready: the step stops without creating anything
    let mut e = Env::default();
    let err = run_step(Item::Registry, &mut e, Duration::from_secs(3), Duration::from_secs(1)).unwrap_err();
    assert!(err.contains("not installed and Ready") && e.created == 0, "{err}");
    // a chosen cluster does not wait for k3s
    let mut e = Env { other: true, ..Default::default() };
    run_step(Item::Registry, &mut e, Duration::from_secs(3), Duration::from_secs(1)).unwrap();
    assert_eq!(e.created, 1);
    // a failure is the step's error and nothing is marked
    let mut e = Env { ready: true, fail: Some("The cluster does not answer".into()), ..Default::default() };
    assert!(run_step(Item::Registry, &mut e, Duration::from_secs(3), Duration::from_secs(1)).unwrap_err().contains("does not answer"));
    assert!(e.marks.is_empty());
}

#[test]
fn flags_and_the_job_command_round_trip() {
    let args: Vec<String> = ["registry", "--data", "/d", "--kubeconfig", "/k", "--context", "lab", "--node-host", "10.1.2.3", "--size", "20", "--port", "30600"].iter().map(|s| s.to_string()).collect();
    let a = registry_args(&args).unwrap();
    assert_eq!((a.size_gi, a.node_port), (20, 30600));
    assert_eq!(a.target.as_ref().unwrap().node_host, "10.1.2.3");
    let l = Launch { exe: "/bin/iyakki".into(), data: "/d".into() };
    let s = registry_spec(&l, &a);
    let Cmd::Argv(v) = &s.cmd else { panic!() };
    assert_eq!(registry_args(v).unwrap(), a);
    assert_eq!(s.workspace, WORKSPACE);
    assert_eq!(item_of(&Job { command: v.join(" "), ..test_job() }), Some(Item::Registry));
    assert!(registry_args(&["registry".into(), "--size".into(), "big".into()]).is_err());
    assert!(registry_args(&["registry".into(), "--port".into(), "99999".into()]).is_err());
    assert_eq!(registry_args(&["registry".into()]).unwrap(), RegistryArgs::default());
}

fn test_job() -> Job {
    Job { id: "j1".into(), label: String::new(), command: String::new(), cwd: "/".into(), owner: "a".into(), workspace: WORKSPACE.into(), pid: 0, state: crate::jobs::State::Running, started_ms: 0, ended_ms: 0, progress: None, last_line: String::new(), dismissed: false }
}

#[test]
fn kubectl_and_k3s_links_point_at_the_managed_program_and_leave_a_real_kubectl_alone() {
    let d = std::env::temp_dir().join(format!("iyakki-qi-links-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let k3s = d.join("k3s-dev/bin/k3s");
    std::fs::create_dir_all(k3s.parent().unwrap()).unwrap();
    std::fs::write(&k3s, "x").unwrap();
    let bin = d.join("cli/bin");
    assert_eq!(link_k3s_tools(&bin, &k3s).unwrap(), ["kubectl", "k3s"]);
    assert_eq!(std::fs::read_link(bin.join("kubectl")).unwrap(), k3s);
    assert!(link_k3s_tools(&bin, &k3s).unwrap().is_empty(), "again: nothing to do");
    // a real file (the CLI catalogue's kubectl) is not replaced
    std::fs::remove_file(bin.join("kubectl")).unwrap();
    std::fs::write(bin.join("kubectl"), "real").unwrap();
    assert!(link_k3s_tools(&bin, &k3s).unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(bin.join("kubectl")).unwrap(), "real");
    // a link of ours to an old place is renewed
    std::fs::remove_file(bin.join("k3s")).unwrap();
    std::os::unix::fs::symlink(d.join("old/k3s-dev/bin/k3s"), bin.join("k3s")).unwrap();
    assert_eq!(link_k3s_tools(&bin, &k3s).unwrap(), ["k3s"]);
    assert!(link_k3s_tools(&bin, &d.join("missing")).unwrap_err().contains("does not exist"));
    let _ = std::fs::remove_dir_all(&d);
}
