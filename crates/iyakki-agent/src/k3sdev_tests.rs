// #[cfg(test)] -- this whole file is test code (k3sdev.rs includes it under #[cfg(test)]); the marker keeps the BOUNDARY check from counting it.
//! Tests of the managed dev cluster: plan text, checksum verification, refusals, start/stop/restart (with a stand-in `k3s` script),
//! reset and uninstall with typed confirmation, health and alerts, the Podman variant, upgrade. No real download, no real cluster.

use super::*;
use crate::archive::tests::tmp;
use crate::wsexec::FakeRunner;
use iyakki_proxy::StreamHead;
use std::collections::HashMap;
use std::io::Cursor;

type Reply = (u16, Vec<(String, String)>, Vec<u8>);

struct Fake {
    routes: Mutex<HashMap<String, Reply>>,
    asked: Mutex<Vec<String>>,
}

impl iyakki_proxy::Streamer for Fake {
    fn open(&self, url: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
        self.asked.lock().unwrap().push(url.to_string());
        let r = self.routes.lock().unwrap().get(url).cloned().ok_or_else(|| format!("no route for {url}"))?;
        let mut headers = r.1.clone();
        headers.push(("content-length".into(), r.2.len().to_string()));
        Ok(StreamHead { status: r.0, headers, body: Box::new(Cursor::new(r.2)) })
    }
}

const VERSION: &str = "v1.31.1+k3s1";

/// A stand-in k3s: `server` writes the kubeconfig it is asked for and stays; `kubectl` is never run (the fake runner answers).
const GOOD_K3S: &str = "#!/bin/sh\ncase \"$1\" in\nserver)\n  kc=\n  while [ $# -gt 0 ]; do if [ \"$1\" = \"--write-kubeconfig\" ]; then kc=\"$2\"; fi; shift; done\n  echo \"fake k3s started\"\n  printf 'apiVersion: v1\\n' > \"$kc\"\n  exec sleep 600;;\nesac\n";
/// …one that dies at once.
const DYING_K3S: &str = "#!/bin/sh\necho \"boom: cannot bind port 6443\"\nexit 3\n";

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn routes(body: &[u8], listed_hash: &str) -> Arc<Fake> {
    let d = clis::def("k3s").unwrap();
    let (clis::Source::File(src), Some(clis::Checksum::Lines(sum))) = (d.source, d.checksum) else { panic!("catalogue") };
    let url = clis::template(src, VERSION).unwrap();
    let sum_url = clis::template(sum, VERSION).unwrap();
    let file = url.rsplit('/').next().unwrap().to_string();
    let mut m: HashMap<String, Reply> = HashMap::new();
    m.insert("https://github.com/k3s-io/k3s/releases/latest/download/k3s".into(), (302, vec![("location".into(), "https://github.com/k3s-io/k3s/releases/download/v1.31.1%2Bk3s1/k3s".into())], vec![]));
    m.insert(url, (200, vec![], body.to_vec()));
    m.insert(sum_url, (200, vec![], format!("{listed_hash}  {file}\n0000000000000000000000000000000000000000000000000000000000000000  other-file\n").into_bytes()));
    Arc::new(Fake { routes: Mutex::new(m), asked: Mutex::new(vec![]) })
}

struct World {
    d: Arc<K3sDev>,
    run: Arc<FakeRunner>,
    dir: PathBuf,
    audit: Arc<Mutex<Vec<(String, String, String)>>>,
}

fn world_with(tag: &str, programs: &[&str]) -> World {
    let dir = tmp(tag);
    let run = Arc::new(FakeRunner::new(programs));
    let clis = Arc::new(CliManager::new(&dir.join("cli")));
    let d = K3sDev::new(&dir.join("data"), clis, run.clone());
    let machine = Machine { k3s_yaml: dir.join("etc/rancher/k3s/k3s.yaml"), lib_dir: dir.join("var/lib/rancher/k3s"), bins: vec![dir.join("usr/local/bin/k3s")], units: vec![], proc_dir: dir.join("proc") };
    std::fs::create_dir_all(&machine.proc_dir).unwrap();
    d.assume(true, true, false, false, machine, Duration::from_millis(10), Duration::from_millis(20));
    d.assume_connected(None);
    let audit: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(vec![]));
    let a2 = audit.clone();
    d.set_audit(Arc::new(move |u, a, x| a2.lock().unwrap().push((u.into(), a.into(), x.into()))));
    World { d, run, dir, audit }
}

fn world(tag: &str) -> World {
    world_with(tag, &[])
}

impl World {
    fn actions(&self) -> Vec<String> {
        self.audit.lock().unwrap().iter().map(|a| a.1.clone()).collect()
    }
    fn plan(&self, st: &Fake, backend: Backend) -> Plan {
        lookup(&self.d.clis, st, &self.d.layout, backend, "in-cluster").unwrap()
    }
    fn nodes_ready(&self) {
        self.run.answer("jsonpath={.items[*].status.conditions", "True");
        self.run.answer("kubeletVersion", "v1.31.1+k3s1");
        self.run.answer("get pods -A", "kube-system coredns-1 1/1 Running 0 1m\nkube-system local-path-1 1/1 Running 0 1m\ndefault job-1 0/1 Pending 0 1m\n");
    }
}

fn wait_for(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

fn set_mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_plan_lists_what_is_downloaded_with_its_checksum_where_it_installs_what_it_starts_the_ports_and_the_registry() {
    let w = world("k3sdev-plan");
    let st = routes(GOOD_K3S.as_bytes(), &sha(GOOD_K3S.as_bytes()));
    let p = w.plan(&st, Backend::Native);
    assert_eq!(p.version, VERSION);
    assert_eq!(p.size, Some(GOOD_K3S.len() as u64));
    assert_eq!(p.expected_sha, sha(GOOD_K3S.as_bytes()), "taken from the vendor's checksum file, for this file only");
    let root = w.dir.join("data/k3s-dev");
    assert_eq!(p.install_path, root.join("bin/k3s"));
    let t = plan_text(&p, true, true);
    for want in [
        "https://github.com/k3s-io/k3s/releases/download/v1.31.1%2Bk3s1/k3s",
        "sha256sum-",
        &sha(GOOD_K3S.as_bytes()),
        "thrown away and nothing is installed",
        &format!("{}/bin/k3s", root.display()),
        &format!("data folder {}/data", root.display()),
        "Reset deletes only this folder",
        "mode 0600",
        "nothing goes to /usr/local, /etc or /var/lib/rancher",
        "--disable traefik",
        "restarted with a back-off",
        "6443/tcp",
        "30000-32767/tcp",
        "A, a registry pod inside the cluster (recommended",
        "separate step you approve",
        "never uses sudo",
        "tmpfs",
    ] {
        assert!(t.contains(want), "the plan should say `{want}`:\n{t}");
    }
    assert!(plan_text(&p, false, false).contains("NOT root"), "a plan for a non-root IYAKKI says so");
    assert!(!plan_text(&p, true, false).contains("mount namespace"), "the RAM-root sentence only when the root is RAM");
    let mut b = p.clone();
    b.registry = "system".into();
    assert!(plan_text(&b, true, false).contains("B, a system registry"));
    // looking up is only reading: nothing is on disk yet
    assert!(!root.exists(), "{:?}", std::fs::read_dir(&root).map(|r| r.count()));
    assert!(w.actions().is_empty());
}

#[test]
fn a_good_download_is_verified_installed_inside_the_data_partition_and_every_step_is_audited() {
    let w = world("k3sdev-good");
    let st = routes(GOOD_K3S.as_bytes(), &sha(GOOD_K3S.as_bytes()));
    let p = w.plan(&st, Backend::Native);
    w.d.install(&p, &*st, "root").unwrap();
    let root = w.dir.join("data/k3s-dev");
    assert_eq!(std::fs::read_to_string(root.join("bin/k3s")).unwrap(), GOOD_K3S);
    assert_eq!(set_mode(&root.join("bin/k3s")), 0o755);
    assert!(!root.join("tmp/k3s.download").exists() && !root.join("bin/k3s.new").exists(), "no half-finished files");
    let s = w.d.installed().unwrap();
    assert_eq!((s.backend, s.version.as_str(), s.sha256.as_str(), s.installed_by.as_str(), s.autostart, s.registry_choice.as_str()), (Backend::Native, VERSION, sha(GOOD_K3S.as_bytes()).as_str(), "root", true, "in-cluster"));
    assert_eq!(w.d.state(), Svc::Stopped, "installed, not started");
    // nothing outside the managed folder was written
    for outside in ["usr", "etc", "var"] {
        assert!(!w.dir.join(outside).exists(), "{outside} must not be touched");
    }
    let a = w.actions();
    for want in ["devcluster.setup", "devcluster.download", "devcluster.verified", "devcluster.installed"] {
        assert!(a.iter().any(|x| x == want), "{want} in {a:?}");
    }
    assert!(w.audit.lock().unwrap().iter().any(|(u, a, x)| u == "root" && a == "devcluster.installed" && x.contains(&sha(GOOD_K3S.as_bytes()))));
    // a second setup is refused: the cluster already is managed
    let e = w.d.install(&p, &*st, "root").unwrap_err();
    assert!(e.contains("already manages"), "{e}");
}

#[test]
fn a_download_that_does_not_match_the_vendors_checksum_is_thrown_away_and_leaves_nothing() {
    let w = world("k3sdev-bad");
    let st = routes(GOOD_K3S.as_bytes(), &sha(GOOD_K3S.as_bytes()));
    let mut p = w.plan(&st, Backend::Native);
    // the server serves other bytes than the ones the checksum file was made for
    st.routes.lock().unwrap().get_mut(&p.url).unwrap().2 = b"#!/bin/sh\necho tampered\n".to_vec();
    let e = w.d.install(&p, &*st, "root").unwrap_err();
    assert!(e.contains("does not match the vendor's checksum") && e.contains("nothing was installed"), "{e}");
    let root = w.dir.join("data/k3s-dev");
    assert!(!root.exists() || std::fs::read_dir(&root).unwrap().next().is_none(), "the managed folder holds nothing: {:?}", std::fs::read_dir(&root).map(|r| r.flatten().map(|e| e.path()).collect::<Vec<_>>()));
    assert!(w.d.installed().is_none());
    assert!(w.actions().contains(&"devcluster.checksum.mismatch".to_string()) && w.actions().contains(&"devcluster.setup.failed".to_string()), "{:?}", w.actions());
    // a plan whose own checksum was tampered with is refused the same way
    p.expected_sha = "f".repeat(64);
    st.routes.lock().unwrap().get_mut(&p.url).unwrap().2 = GOOD_K3S.as_bytes().to_vec();
    assert!(w.d.install(&p, &*st, "root").unwrap_err().contains("does not match"));
    // a checksum file with no entry for the file is an error, not a pass
    let st2 = routes(GOOD_K3S.as_bytes(), &sha(GOOD_K3S.as_bytes()));
    let sum_url = p.checksum_url.clone();
    st2.routes.lock().unwrap().insert(sum_url, (200, vec![], b"0000000000000000000000000000000000000000000000000000000000000000  something-else\n".to_vec()));
    assert!(lookup(&w.d.clis, &*st2, &w.d.layout, Backend::Native, "").unwrap_err().contains("no entry"));
}

#[test]
fn setup_is_refused_when_a_cluster_exists_when_not_root_in_a_window_or_on_a_memory_disk_and_nothing_is_changed() {
    let st = routes(GOOD_K3S.as_bytes(), &sha(GOOD_K3S.as_bytes()));
    // k3s already installed (its config file)
    let w = world("k3sdev-exists1");
    let p = w.plan(&st, Backend::Native);
    std::fs::create_dir_all(w.dir.join("etc/rancher/k3s")).unwrap();
    std::fs::write(w.dir.join("etc/rancher/k3s/k3s.yaml"), "x").unwrap();
    let e = w.d.install(&p, &*st, "root").unwrap_err();
    assert!(e.contains("already has a cluster") && e.contains("k3s.yaml") && e.contains("Nothing was changed"), "{e}");
    assert!(!w.dir.join("data/k3s-dev").exists());
    assert!(w.actions().contains(&"devcluster.setup.refused".to_string()));
    // k3s running (a process)
    let w = world("k3sdev-exists2");
    let p = w.plan(&st, Backend::Native);
    let proc1 = w.dir.join("proc/4242");
    std::fs::create_dir_all(&proc1).unwrap();
    std::fs::write(proc1.join("comm"), "k3s-server\n").unwrap();
    let e = w.d.install(&p, &*st, "root").unwrap_err();
    assert!(e.contains("a process named k3s-server is running"), "{e}");
    // a system-wide k3s program, a service file, a connected cluster, the sandbox cluster
    let mut cases: Vec<(&str, Box<dyn Fn(&World)>)> = Vec::new();
    cases.push(("bin", Box::new(|w: &World| {
        std::fs::create_dir_all(w.dir.join("usr/local/bin")).unwrap();
        std::fs::write(w.dir.join("usr/local/bin/k3s"), "x").unwrap();
    })));
    cases.push(("lib", Box::new(|w: &World| std::fs::create_dir_all(w.dir.join("var/lib/rancher/k3s")).unwrap())));
    cases.push(("conn", Box::new(|w: &World| w.d.assume_connected(Some("k3s (kubeconfig /etc/rancher/k3s/k3s.yaml)")))));
    cases.push(("sandbox", Box::new(|w: &World| w.d.set_sandbox_probe(Arc::new(|| true)))));
    for (tag, make) in cases {
        let w = world(&format!("k3sdev-exists-{tag}"));
        let p = w.plan(&st, Backend::Native);
        make(&w);
        assert!(w.d.install(&p, &*st, "root").unwrap_err().contains("will not set up another one"), "{tag}");
        assert!(w.d.installed().is_none());
    }
    // the CLI tile's own k3s program does not count as a cluster
    let w = world("k3sdev-clitile");
    clis::write_fake_installed(&w.d.clis, "k3s", "#!/bin/sh\n");
    assert!(w.d.found_elsewhere().is_empty(), "{:?}", w.d.found_elsewhere());
    // not root, no cgroup v2
    let w = world("k3sdev-root");
    let p = w.plan(&st, Backend::Native);
    let m = w.d.machine.lock().unwrap().clone();
    w.d.assume(false, false, false, false, m.clone(), Duration::from_millis(10), Duration::from_millis(20));
    let e = w.d.install(&p, &*st, "root").unwrap_err();
    assert!(e.contains("not running as root") && e.contains("cgroup v2"), "{e}");
    // a desktop window never installs, whatever else is true
    w.d.assume(true, true, false, true, m, Duration::from_millis(10), Duration::from_millis(20));
    let e = w.d.install(&p, &*st, "root").unwrap_err();
    assert!(e.contains("desktop window"), "{e}");
    assert!(!w.dir.join("data/k3s-dev").exists());
    // a data folder in memory
    let mounts = "rootfs / rootfs rw 0 0\n/dev/sda2 /var/lib/iyakki ext4 rw 0 0\ntmpfs /run tmpfs rw 0 0\n";
    assert_eq!(fs_type_of(Path::new("/var/lib/iyakki/k3s-dev"), mounts).as_deref(), Some("ext4"));
    assert_eq!(fs_type_of(Path::new("/var/lib/other"), mounts).as_deref(), Some("rootfs"));
    assert_eq!(fs_type_of(Path::new("/run/x"), mounts).as_deref(), Some("tmpfs"));
}

#[test]
fn start_stop_and_restart_run_the_program_as_a_managed_service_the_node_becomes_ready_and_each_is_audited() {
    let w = world("k3sdev-life");
    let st = routes(GOOD_K3S.as_bytes(), &sha(GOOD_K3S.as_bytes()));
    w.d.install(&w.plan(&st, Backend::Native), &*st, "root").unwrap();
    w.nodes_ready();
    assert!(w.d.stop("root").is_err(), "not running yet");
    w.d.start("root").unwrap();
    assert!(w.d.start("root").unwrap_err().contains("already running"));
    assert!(wait_for(10, || w.d.state() == Svc::Running), "{:?}", w.d.state());
    let kc = w.d.layout.kubeconfig();
    assert!(kc.exists(), "the program wrote the kubeconfig it was asked for");
    assert_eq!(set_mode(&kc), 0o600);
    assert!(std::fs::read_to_string(w.d.layout.log()).unwrap().contains("fake k3s started"), "its output is the log");
    // the command line is exactly the plan's
    let h = w.d.health_now();
    assert_eq!((h.state.as_str(), h.version.as_str(), h.node_ready, h.pods), ("running", "v1.31.1+k3s1", Some(true), Some((2, 3))));
    assert!(h.line().contains("running") && h.line().contains("node Ready") && h.line().contains("2/3 pods"), "{}", h.line());
    let k = w.run.calls().into_iter().find(|c| c.argv.contains(&"kubectl".to_string())).unwrap();
    assert_eq!(k.argv[0], w.d.layout.bin().display().to_string(), "kubectl is `k3s kubectl`, no separate program needed");
    assert!(k.argv.contains(&"--kubeconfig".to_string()) && k.env.iter().any(|(n, v)| n == "KUBECONFIG" && v == &kc.display().to_string()));
    // restart
    w.d.restart("root").unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running), "{:?}", w.d.state());
    w.d.stop("root").unwrap();
    assert_eq!(w.d.state(), Svc::Stopped);
    assert!(!kc.exists(), "the stale kubeconfig is removed");
    let a = w.actions();
    for want in ["devcluster.start", "devcluster.ready", "devcluster.restart.requested", "devcluster.stop"] {
        assert!(a.iter().any(|x| x == want), "{want} in {a:?}");
    }
    // the log can be read as text
    assert!(w.d.log_tail(10).contains("fake k3s started"));
}

#[test]
fn a_cluster_that_keeps_dying_is_restarted_with_a_back_off_then_given_up_and_the_administrators_are_told() {
    let w = world("k3sdev-dying");
    let st = routes(DYING_K3S.as_bytes(), &sha(DYING_K3S.as_bytes()));
    w.d.install(&w.plan(&st, Backend::Native), &*st, "root").unwrap();
    w.d.start("root").unwrap();
    assert!(wait_for(20, || matches!(w.d.state(), Svc::Failed(_))), "{:?}", w.d.state());
    let Svc::Failed(why) = w.d.state() else { unreachable!() };
    assert!(why.contains("boom") && why.contains("stopped trying"), "{why}");
    let s = w.d.installed().unwrap();
    assert_eq!(s.restarts as usize, MAX_FAILURES, "restarted until the limit");
    assert!(s.last_restart_reason.contains("boom: cannot bind port 6443"), "{}", s.last_restart_reason);
    let alerts = w.d.take_alerts();
    assert!(alerts.iter().any(|a| matches!(a, Alert::Down(r) if r.contains("boom"))), "{alerts:?}");
    assert!(alerts.iter().any(|a| matches!(a, Alert::GaveUp(r) if r.contains("stopped trying"))), "{alerts:?}");
    assert_eq!(alerts.iter().filter(|a| matches!(a, Alert::Down(_))).count(), 1, "one announcement per outage, not one per restart: {alerts:?}");
    assert!(w.d.take_alerts().is_empty(), "taken once");
    let (title, body) = alerts[0].title_body();
    assert!(title.contains("went down") && body.contains("Settings"));
    assert!(alerts[1].title_body().0.contains("keeps failing"));
    let a = w.actions();
    assert!(a.iter().filter(|x| *x == "devcluster.restart").count() >= 2 && a.contains(&"devcluster.exited".to_string()), "{a:?}");
    let h = w.d.health();
    assert_eq!((h.state.as_str(), h.restarts), ("failed", MAX_FAILURES as u32));
    assert!(h.failed_reason.contains("boom") && h.last_restart_reason.contains("boom"));
}

#[test]
fn reset_needs_the_typed_node_name_and_deletes_only_the_data_folder_uninstall_removes_the_managed_folder_only() {
    let w = world("k3sdev-reset");
    let st = routes(GOOD_K3S.as_bytes(), &sha(GOOD_K3S.as_bytes()));
    w.d.install(&w.plan(&st, Backend::Native), &*st, "root").unwrap();
    let root = w.dir.join("data/k3s-dev");
    // something of the person's next to the managed folder, and something inside the data folder
    std::fs::write(w.dir.join("data/precious.txt"), "mine").unwrap();
    std::fs::create_dir_all(root.join("data/server/db")).unwrap();
    std::fs::write(root.join("data/server/db/state.db"), "etcd").unwrap();
    // wrong confirmations change nothing
    for typed in ["", "reset", "yes", "iyakki", "IYAKKI-DEV"] {
        let e = w.d.reset(typed, "root").unwrap_err();
        assert!(e.contains("Type iyakki-dev") && e.contains("nothing was changed"), "{typed}: {e}");
    }
    assert!(root.join("data/server/db/state.db").exists());
    assert!(!w.actions().contains(&"devcluster.reset".to_string()), "nothing audited as done for a refused reset");
    w.d.reset("iyakki-dev", "root").unwrap();
    assert!(!root.join("data/server").exists() && root.join("data").is_dir(), "the data folder is empty again");
    assert!(root.join("bin/k3s").exists() && root.join("state.json").exists(), "the program and the state stay");
    assert_eq!(std::fs::read_to_string(w.dir.join("data/precious.txt")).unwrap(), "mine");
    assert!(w.actions().contains(&"devcluster.reset".to_string()) && w.actions().contains(&"devcluster.reset.done".to_string()));
    // a link in place of the data folder is not followed
    std::fs::remove_dir_all(root.join("data")).unwrap();
    let outside = w.dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep"), "x").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("data")).unwrap();
    let e = w.d.reset("iyakki-dev", "root").unwrap_err();
    assert!(e.contains("is a link"), "{e}");
    assert!(outside.join("keep").exists());
    std::fs::remove_file(root.join("data")).unwrap();
    // uninstall
    assert!(w.d.uninstall("nope", "root").unwrap_err().contains("Type iyakki-dev"));
    assert!(root.exists());
    w.d.uninstall("iyakki-dev", "root").unwrap();
    assert!(!root.exists(), "the managed folder is gone");
    assert!(w.dir.join("data/precious.txt").exists(), "…and nothing else");
    assert_eq!(w.d.state(), Svc::NotInstalled);
    assert!(w.d.installed().is_none());
    assert!(w.actions().contains(&"devcluster.uninstall.done".to_string()));
}

#[test]
fn reset_and_uninstall_stop_a_running_cluster_first() {
    let w = world("k3sdev-resetrun");
    let st = routes(GOOD_K3S.as_bytes(), &sha(GOOD_K3S.as_bytes()));
    w.d.install(&w.plan(&st, Backend::Native), &*st, "root").unwrap();
    w.nodes_ready();
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running));
    w.d.reset("iyakki-dev", "root").unwrap();
    assert_eq!(w.d.state(), Svc::Stopped);
    assert!(w.actions().contains(&"devcluster.stop".to_string()));
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running));
    w.d.uninstall("iyakki-dev", "root").unwrap();
    assert!(!w.dir.join("data/k3s-dev").exists());
}

#[test]
fn health_counts_pods_measures_the_data_folder_and_the_disk_alert_fires_once_above_85_percent() {
    let w = world("k3sdev-health");
    let st = routes(GOOD_K3S.as_bytes(), &sha(GOOD_K3S.as_bytes()));
    assert!(!w.d.health().installed);
    assert_eq!(w.d.health().line(), "not installed");
    w.d.install(&w.plan(&st, Backend::Native), &*st, "root").unwrap();
    std::fs::write(w.d.layout.data().join("blob"), vec![1u8; 4096]).unwrap();
    let h = w.d.health_now();
    assert_eq!((h.installed, h.state.as_str(), h.version.as_str()), (true, "stopped", VERSION), "stopped: the installed version");
    assert!(h.data_bytes.unwrap() >= 4096);
    assert!(h.disk_pct.is_some() && h.disk_free.is_some(), "the partition of the data folder is measured");
    // alerts from the disk figure
    let mut full = h.clone();
    full.disk_pct = Some(90);
    w.d.check_disk(&full);
    w.d.check_disk(&full);
    assert_eq!(w.d.take_alerts(), vec![Alert::Disk(90)], "once, not at every look");
    full.disk_pct = Some(83);
    w.d.check_disk(&full);
    assert!(w.d.take_alerts().is_empty(), "between 80 and 85 it stays quiet and does not re-arm");
    full.disk_pct = Some(70);
    w.d.check_disk(&full);
    full.disk_pct = Some(86);
    w.d.check_disk(&full);
    assert_eq!(w.d.take_alerts(), vec![Alert::Disk(86)], "armed again after it dropped below 80");
    let (t, b) = Alert::Disk(86).title_body();
    assert!(t.contains("86%") && b.contains("85%"));
    // a node that is not Ready shows in the line
    w.run.answer("jsonpath={.items[*].status.conditions", "False");
    w.d.install_fake_running_for_test();
    let h = w.d.health_now();
    assert_eq!(h.node_ready, Some(false));
    assert!(h.line().contains("node NOT ready"), "{}", h.line());
}

impl K3sDev {
    fn install_fake_running_for_test(&self) {
        self.set_svc(Svc::Running);
    }
}

#[test]
fn the_podman_variant_needs_podman_pulls_by_tag_records_the_digest_and_is_controlled_through_the_runner() {
    let st = routes(b"unused", &"0".repeat(64));
    // no podman: refused with the reason, nothing recorded
    let w = world("k3sdev-nopodman");
    let p = w.plan(&st, Backend::Podman);
    assert_eq!(p.image, "docker.io/rancher/k3s:v1.31.1-k3s1");
    assert!(p.size.is_none() && p.expected_sha.is_empty());
    let t = plan_text(&p, true, false);
    for want in ["podman pull docker.io/rancher/k3s:v1.31.1-k3s1", "podman create --name iyakki-k3s --privileged --network=host", "/var/lib/rancher/k3s", "by podman, not by IYAKKI", "no separately published checksum"] {
        assert!(t.contains(want), "{want}:\n{t}");
    }
    let e = w.d.install(&p, &*st, "root").unwrap_err();
    assert!(e.contains("podman is not installed") && e.contains("podman packages"), "{e}");
    assert!(w.d.installed().is_none());
    // with podman (fake runner)
    let w = world_with("k3sdev-podman", &["podman"]);
    let p = w.plan(&st, Backend::Podman);
    w.run.answer("image inspect", "sha256:abc123\n");
    w.run.answer("podman pull", "pulled");
    w.run.answer("podman rm", "");
    w.run.answer("podman create", "container-id");
    w.d.install(&p, &*st, "root").unwrap();
    let s = w.d.installed().unwrap();
    assert_eq!((s.backend, s.image.as_str(), s.image_digest.as_str()), (Backend::Podman, "docker.io/rancher/k3s:v1.31.1-k3s1", "sha256:abc123"));
    let lines = w.run.lines();
    assert!(lines.iter().any(|l| l.starts_with("podman pull ")) && lines.iter().any(|l| l.starts_with("podman create ")), "{lines:?}");
    let create = lines.iter().find(|l| l.starts_with("podman create ")).unwrap();
    assert!(create.contains(&format!("{}/data:/var/lib/rancher/k3s", w.dir.join("data/k3s-dev").display())) && create.contains("K3S_KUBECONFIG_MODE=600"), "{create}");
    assert!(!w.dir.join("data/k3s-dev/bin/k3s").exists(), "no native program for this variant");
    // start, health through podman exec, stop, logs
    w.run.answer("podman inspect --format", "running 0");
    w.run.answer("podman inspect iyakki-k3s", "[{}]");
    w.run.answer("podman start", "iyakki-k3s");
    w.run.answer("podman exec iyakki-k3s kubectl --request-timeout=10s get nodes -o jsonpath={.items[*].status.conditions", "True");
    w.run.answer("podman exec", "v1.31.1+k3s1");
    w.run.answer("podman stop", "iyakki-k3s");
    w.run.answer("podman logs", "k3s log line from the container");
    // the container is "started" by writing the kubeconfig the container would write
    w.d.start("root").unwrap();
    std::fs::write(w.d.layout.kubeconfig(), "x").unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running), "{:?}", w.d.state());
    assert!(w.run.lines().iter().any(|l| l == "podman start iyakki-k3s"));
    assert!(w.d.log_tail(5).contains("container"));
    w.d.stop("root").unwrap();
    assert!(w.run.lines().iter().any(|l| l == "podman stop -t 30 iyakki-k3s"));
    w.d.uninstall("iyakki-dev", "root").unwrap();
    assert!(w.run.lines().iter().any(|l| l == "podman rm -f iyakki-k3s"), "only the container of this name is removed");
}

#[test]
fn an_upgrade_downloads_and_verifies_first_then_replaces_the_program_and_keeps_the_old_one() {
    let w = world("k3sdev-upgrade");
    let st = routes(GOOD_K3S.as_bytes(), &sha(GOOD_K3S.as_bytes()));
    w.d.install(&w.plan(&st, Backend::Native), &*st, "root").unwrap();
    assert!(w.d.upgrade_plan(&*st).unwrap().is_none(), "already the latest");
    // a newer release appears
    let newer = "#!/bin/sh\n# newer\nexec sleep 600\n";
    let v2 = "v1.32.0+k3s1";
    let d = clis::def("k3s").unwrap();
    let (clis::Source::File(src), Some(clis::Checksum::Lines(sum))) = (d.source, d.checksum) else { panic!() };
    let (url2, sum2) = (clis::template(src, v2).unwrap(), clis::template(sum, v2).unwrap());
    let file = url2.rsplit('/').next().unwrap().to_string();
    {
        let mut r = st.routes.lock().unwrap();
        r.insert("https://github.com/k3s-io/k3s/releases/latest/download/k3s".into(), (302, vec![("location".into(), "https://github.com/k3s-io/k3s/releases/download/v1.32.0%2Bk3s1/k3s".into())], vec![]));
        r.insert(url2.clone(), (200, vec![], newer.as_bytes().to_vec()));
        r.insert(sum2, (200, vec![], format!("{}  {file}\n", sha(newer.as_bytes())).into_bytes()));
    }
    let p = w.d.upgrade_plan(&*st).unwrap().expect("a newer version");
    assert_eq!(p.version, v2);
    // a bad download changes nothing
    st.routes.lock().unwrap().get_mut(&url2).unwrap().2 = b"#!/bin/sh\necho evil\n".to_vec();
    assert!(w.d.upgrade(&p, &*st, "root").unwrap_err().contains("does not match"));
    let bin = w.d.layout.bin();
    assert_eq!(std::fs::read_to_string(&bin).unwrap(), GOOD_K3S, "the old program is untouched");
    assert_eq!(w.d.installed().unwrap().version, VERSION);
    // the good one replaces it
    st.routes.lock().unwrap().get_mut(&url2).unwrap().2 = newer.as_bytes().to_vec();
    w.d.upgrade(&p, &*st, "root").unwrap();
    assert_eq!(std::fs::read_to_string(&bin).unwrap(), newer);
    assert_eq!(std::fs::read_to_string(w.dir.join("data/k3s-dev/bin/k3s.prev")).unwrap(), GOOD_K3S, "the old program is kept");
    assert_eq!(set_mode(&bin), 0o755);
    assert_eq!(w.d.installed().unwrap().version, v2);
    assert!(w.actions().contains(&"devcluster.upgraded".to_string()));
}

#[test]
fn autostart_starts_an_installed_cluster_once_unless_it_is_switched_off_or_this_is_a_window_or_something_else_runs() {
    let w = world("k3sdev-auto");
    w.d.autostart();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(w.d.state(), Svc::NotInstalled, "nothing installed: nothing to do, quietly");
    let st = routes(GOOD_K3S.as_bytes(), &sha(GOOD_K3S.as_bytes()));
    w.d.install(&w.plan(&st, Backend::Native), &*st, "root").unwrap();
    w.nodes_ready();
    w.d.set_autostart(false, "root").unwrap();
    w.d.autostart();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(w.d.state(), Svc::Stopped, "Autostart off");
    w.d.set_autostart(true, "root").unwrap();
    // something else is running: not started, the administrators are told
    w.d.assume_connected(Some("a cluster"));
    w.d.autostart();
    assert!(wait_for(5, || !w.d.take_alerts().is_empty()));
    assert_eq!(w.d.state(), Svc::Stopped);
    assert!(w.actions().contains(&"devcluster.autostart.failed".to_string()));
    w.d.assume_connected(None);
    w.d.autostart();
    assert!(wait_for(10, || w.d.state() == Svc::Running), "{:?}", w.d.state());
    w.d.stop("system").unwrap();
    // a window never starts it
    let m = w.d.machine.lock().unwrap().clone();
    w.d.assume(true, true, false, true, m, Duration::from_millis(10), Duration::from_millis(20));
    w.d.assume_connected(None);
    w.d.autostart();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(w.d.state(), Svc::Stopped);
    assert!(w.d.start("root").unwrap_err().contains("desktop window"));
    // the registry choice is remembered and checked
    w.d.set_registry_choice("system", "root").unwrap();
    assert_eq!(w.d.installed().unwrap().registry_choice, "system");
    assert!(w.d.set_registry_choice("bogus", "root").is_err());
}
