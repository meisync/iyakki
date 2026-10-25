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
    cases.push(("lib", Box::new(|w: &World| std::fs::create_dir_all(w.dir.join("var/lib/rancher/k3s/server")).unwrap())));
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

// ------------------------------------------------------------------------------------------------ waiting for the network

const ROUTE_HEAD: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n";
const ROUTE_DEFAULT: &str = "eth0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n";
const ROUTE_LAN: &str = "eth0\t0000A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";

fn route_text(body: &str) -> String {
    format!("{ROUTE_HEAD}{body}")
}

#[test]
fn the_route_file_parser_finds_an_ipv4_default_route_on_an_interface_that_is_up() {
    assert!(has_default_route(&route_text(&format!("{ROUTE_LAN}{ROUTE_DEFAULT}"))));
    assert!(!has_default_route(&route_text(ROUTE_LAN)), "only the local network");
    assert!(!has_default_route(ROUTE_HEAD), "header only");
    assert!(!has_default_route(""), "empty or missing file");
    assert!(!has_default_route(&route_text("lo\t00000000\t00000000\t0001\t0\t0\t0\t00000000\t0\t0\t0\n")), "the loopback does not count");
    assert!(!has_default_route(&route_text("eth0\t00000000\t0100A8C0\t0002\t0\t0\t100\t00000000\t0\t0\t0\n")), "gateway flag but not up");
    assert!(!has_default_route(&route_text("eth0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00FFFFFF\t0\t0\t0\n")), "a netmask other than 0 is not a default route");
    // IPv6 only: /proc/net/ipv6_route lines (32 hex digit addresses) are not IPv4 default routes
    assert!(!has_default_route(&route_text("00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000064 00000001 00000000 00000003 eth0\n")));
    // malformed lines are skipped, a good one after them still counts
    assert!(!has_default_route(&route_text("garbage\neth0 00000000\nzz zz zz zz zz zz zz zz\n")));
    assert!(has_default_route(&route_text(&format!("garbage\n{ROUTE_DEFAULT}"))));
    assert!(has_default_route(&route_text("wlan0 00000000 0100A8C0 0003 0 0 600 00000000 0 0 0\n")), "spaces instead of tabs");
}

#[test]
fn the_wait_ends_when_a_route_appears_says_no_network_yet_after_the_grace_time_and_never_gives_up() {
    // fake clock: every sleep is one step; the route appears after 40 steps of 3 s = 120 s
    let mut steps = 0u32;
    let mut notes = vec![];
    let r = {
        let steps_cell = std::cell::Cell::new(0u32);
        let r = wait_for_route(|| steps_cell.get() >= 40, |_| { steps_cell.set(steps_cell.get() + 1); true }, |n| notes.push((n, steps_cell.get())), Duration::from_secs(90), Duration::from_secs(3));
        steps = steps_cell.get().max(steps);
        r
    };
    assert_eq!(r, NetWait::Ready);
    assert_eq!(steps, 40, "no sleep after the route showed up");
    assert_eq!(notes, vec![(NetNote::Waiting, 0), (NetNote::NoNetworkYet, 30)], "waiting at once, no network yet after 90 s, each said once");
    // a route that is there at once: no sleep at all
    let mut slept = 0;
    assert_eq!(wait_for_route(|| true, |_| { slept += 1; true }, |_| {}, Duration::from_secs(90), Duration::from_secs(3)), NetWait::Ready);
    assert_eq!(slept, 0);
    // cancelled (Stop) while waiting
    let mut n = 0;
    let r = wait_for_route(|| false, |_| { n += 1; n < 5 }, |_| {}, Duration::from_secs(90), Duration::from_secs(3));
    assert_eq!(r, NetWait::Cancelled);
    assert_eq!(n, 5);
}

fn world_net(tag: &str, k3s: &str, with_route: bool) -> (World, PathBuf) {
    let w = world(tag);
    let st = routes(k3s.as_bytes(), &sha(k3s.as_bytes()));
    w.d.install(&w.plan(&st, Backend::Native), &*st, "root").unwrap();
    let route = w.dir.join("route");
    std::fs::write(&route, route_text(if with_route { ROUTE_DEFAULT } else { ROUTE_LAN })).unwrap();
    w.d.assume_route_file(&route, Duration::from_millis(100));
    (w, route)
}

#[test]
fn start_without_a_default_route_waits_shows_it_then_starts_when_the_route_appears_and_counts_nothing() {
    let (w, route) = world_net("k3sdev-netwait", GOOD_K3S, false);
    w.nodes_ready();
    w.d.start("root").unwrap();
    assert!(wait_for(5, || w.d.health().note == WAITING_NET || w.d.health().note == NO_NET_YET), "{:?}", w.d.health());
    assert!(lock(&w.d.child).is_none(), "k3s is not started without a route");
    let h = w.d.health();
    assert_eq!(h.state, "starting");
    assert!(h.line().contains("starting") && h.line().contains("waiting for the network"), "{}", h.line());
    // after the grace time: a waiting state with the reason, not a failure
    assert!(wait_for(5, || w.d.health().note == NO_NET_YET), "{:?}", w.d.health().note);
    assert!(w.d.health().line().contains("needs a network interface with a default route"));
    assert_eq!(w.d.state(), Svc::Starting);
    assert!(w.d.take_alerts().is_empty(), "no GaveUp, no Down");
    assert!(std::fs::read_to_string(w.d.layout.log()).unwrap().contains("iyakki: waiting for the network"), "the log says it too");
    // DHCP finishes
    std::fs::write(&route, route_text(ROUTE_DEFAULT)).unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running), "{:?}", w.d.state());
    let h = w.d.health();
    assert!(h.note.is_empty() && h.restarts == 0, "{h:?}");
    w.d.stop("root").unwrap();
}

#[test]
fn stop_cancels_the_wait_for_the_network() {
    let (w, _route) = world_net("k3sdev-netstop", GOOD_K3S, false);
    w.d.start("root").unwrap();
    assert!(wait_for(5, || !w.d.health().note.is_empty()));
    w.d.stop("root").expect("Stop while waiting is fine");
    assert_eq!(w.d.state(), Svc::Stopped);
    assert!(w.d.health().note.is_empty());
    std::thread::sleep(Duration::from_millis(200));
    assert!(lock(&w.d.child).is_none(), "nothing was started after Stop");
    assert_eq!(w.d.state(), Svc::Stopped);
}

#[test]
fn k3s_dying_while_the_route_is_missing_is_not_a_failure_and_a_real_crash_with_a_route_still_gives_up() {
    // the stand-in removes the route file as it dies, like a lost lease
    let route_placeholder = "ROUTEFILE";
    let script = format!("#!/bin/sh\necho 'level=fatal msg=\"Error: no default routes found in /proc/net/route\"'\nrm -f {route_placeholder}\nexit 1\n");
    // the path is only known after the world exists: build the script text with a fixed path under /tmp-less test dir
    let dir_probe = world("k3sdev-netdie-probe").dir;
    let route = dir_probe.join("route");
    let script = script.replace(route_placeholder, &route.display().to_string());
    let (w, _) = world_net("k3sdev-netdie", &script, true);
    // use the probe dir's route file (the script removes exactly that one)
    std::fs::write(&route, route_text(ROUTE_DEFAULT)).unwrap();
    w.d.assume_route_file(&route, Duration::from_millis(100));
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.d.health().note == WAITING_NET || w.d.health().note == NO_NET_YET), "{:?}", w.d.health());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(w.d.state(), Svc::Starting, "waiting, not Failed");
    assert_eq!(w.d.installed().unwrap().restarts, 0, "waiting for the network is not a restart");
    assert!(w.d.take_alerts().is_empty(), "no Down and no GaveUp while there is no network");
    assert!(w.actions().iter().any(|a| a == "devcluster.exited-no-network"));
    w.d.stop("root").unwrap();
    // with a route that stays, real crashes still count and end in GaveUp (the existing test above covers the full text)
    let w2 = world("k3sdev-netcrash");
    let st = routes(DYING_K3S.as_bytes(), &sha(DYING_K3S.as_bytes()));
    w2.d.install(&w2.plan(&st, Backend::Native), &*st, "root").unwrap();
    let r2 = w2.dir.join("route");
    std::fs::write(&r2, route_text(ROUTE_DEFAULT)).unwrap();
    w2.d.assume_route_file(&r2, Duration::from_millis(100));
    w2.d.start("root").unwrap();
    assert!(wait_for(20, || matches!(w2.d.state(), Svc::Failed(_))), "{:?}", w2.d.state());
    assert!(w2.d.take_alerts().iter().any(|a| matches!(a, Alert::GaveUp(_))));
}

#[test]
fn a_rancher_folder_with_only_unpacked_binaries_is_not_a_cluster_but_server_state_is() {
    let w = world("lib-data-only");
    let lib = w.dir.join("var/lib/rancher/k3s");
    std::fs::create_dir_all(lib.join("data/abc/bin")).unwrap();
    std::fs::write(lib.join("data/abc/bin/runc"), b"x").unwrap();
    let m = Machine { k3s_yaml: w.dir.join("etc/rancher/k3s/k3s.yaml"), lib_dir: lib.clone(), bins: vec![], units: vec![], proc_dir: w.dir.join("proc") };
    assert!(existing(&m, &[], None, false).is_empty(), "only the unpacked binaries: no cluster");
    std::fs::create_dir_all(lib.join("server/cred")).unwrap();
    let v = existing(&m, &[], None, false);
    assert!(v.iter().any(|x| x.contains("k3s has run here")), "{v:?}");
    std::fs::remove_dir_all(lib.join("server")).unwrap();
    std::fs::create_dir_all(lib.join("agent")).unwrap();
    assert!(!existing(&m, &[], None, false).is_empty(), "an agent folder is a cluster");
}

// ------------------------------------------------------------------------------------------------ a supervisor that never goes quiet

fn running_world(tag: &str) -> World {
    let (w, _route) = world_net(tag, GOOD_K3S, true);
    w.nodes_ready();
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running), "{:?}", w.d.state());
    w
}

fn try_pid(w: &World) -> Option<u32> {
    lock(&w.d.child).as_ref().map(|c| c.id())
}

fn child_pid(w: &World) -> u32 {
    lock(&w.d.child).as_ref().expect("a child").id()
}

#[test]
fn two_starts_at_boot_make_one_k3s_not_two() {
    let (w, _route) = world_net("k3sdev-twostarts", GOOD_K3S, true);
    w.nodes_ready();
    let (a, b) = (w.d.clone(), w.d.clone());
    let (t1, t2) = (std::thread::spawn(move || a.start("system")), std::thread::spawn(move || b.start("shell")));
    let (r1, r2) = (t1.join().unwrap(), t2.join().unwrap());
    assert!(r1.is_ok() != r2.is_ok(), "exactly one start wins: {r1:?} {r2:?}");
    assert!(wait_for(10, || w.d.state() == Svc::Running));
    let pid = child_pid(&w);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(child_pid(&w), pid, "the one child is still the one tracked");
    assert_eq!(w.actions().iter().filter(|a| *a == "devcluster.start").count(), 1);
    w.d.stop("root").unwrap();
}

#[test]
fn a_k3s_killed_by_a_signal_is_noticed_logged_with_the_signal_and_restarted() {
    let w = running_world("k3sdev-signal");
    let pid = child_pid(&w);
    unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    assert!(wait_for(10, || w.actions().iter().any(|a| a == "devcluster.exited")), "{:?}", w.actions());
    let log = std::fs::read_to_string(w.d.layout.log()).unwrap();
    assert!(log.contains("iyakki: k3s exited (signal 9 (SIGKILL))"), "{log}");
    assert!(w.audit.lock().unwrap().iter().any(|a| a.1 == "devcluster.exited" && a.2.contains("signal 9")));
    assert!(wait_for(10, || w.d.state() == Svc::Running && try_pid(&w).is_some_and(|p| p != pid)), "restarted: {:?}", w.d.state());
    assert_eq!(w.d.installed().unwrap().restarts, 1);
    w.d.stop("root").unwrap();
}

#[test]
fn a_lost_child_handle_is_a_death_not_a_silent_exit_and_a_panicking_supervisor_never_leaves_running() {
    let w = running_world("k3sdev-lost");
    let pid = child_pid(&w);
    // the handle disappears while the process is gone (what the Mac showed: Running, no process, no audit)
    let ch = lock(&w.d.child).take();
    drop(ch);
    unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    assert!(wait_for(10, || w.actions().iter().any(|a| a == "devcluster.exited")), "{:?}", w.actions());
    assert!(wait_for(10, || w.d.state() == Svc::Running && lock(&w.d.child).is_some()), "{:?}", w.d.state());
    w.d.stop("root").unwrap();
    // every exit is audited with its reason; one that happens while the cluster should run turns into Failed plus an alert
    let a = w.audit.lock().unwrap().clone();
    assert!(a.iter().any(|x| x.1 == "devcluster.supervisor-stopped"), "{a:?}");
    let w = world_net("k3sdev-panic", GOOD_K3S, true).0;
    w.nodes_ready();
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running));
    // a panic in the supervisor: the health call runs kubectl through the fake runner, so make that panic
    w.run.answer("get pods -A", "x");
    let d = w.d.clone();
    let gen = d.gen.load(Ordering::SeqCst);
    d.monitor_panic_for_test(gen);
    assert!(matches!(w.d.state(), Svc::Failed(ref why) if why.contains("stopped watching")), "{:?}", w.d.state());
    assert!(w.d.take_alerts().iter().any(|a| matches!(a, Alert::GaveUp(r) if r.contains("panicked"))));
    assert!(w.audit.lock().unwrap().iter().any(|x| x.1 == "devcluster.supervisor-stopped" && x.2.contains("panicked")));
    let _ = w.d.stop("root");
}

#[test]
fn the_watchdog_takes_over_from_a_supervisor_that_stalled_while_k3s_died() {
    let w = running_world("k3sdev-watchdog");
    let pid = child_pid(&w);
    // the supervisor sleeps for an hour (stalled) while k3s is killed
    lock(&w.d.over).poll = Some(Duration::from_secs(3600));
    std::thread::sleep(Duration::from_millis(300));
    unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    std::thread::sleep(Duration::from_millis(200));
    let gen = w.d.gen.load(Ordering::SeqCst);
    assert!(!w.d.watchdog_step(gen, Backend::Native, Duration::from_secs(3600)), "a fresh heartbeat: leave it alone");
    assert!(w.d.watchdog_step(gen, Backend::Native, Duration::ZERO), "stale: take over");
    assert!(w.actions().iter().any(|a| a == "devcluster.supervisor-stalled") && w.actions().iter().any(|a| a == "devcluster.exited"), "{:?}", w.actions());
    assert!(std::fs::read_to_string(w.d.layout.log()).unwrap().contains("iyakki: k3s exited (signal 9"));
    assert_ne!(child_pid(&w), pid, "started again");
    assert_eq!(w.d.installed().unwrap().restarts, 1);
    assert!(w.audit.lock().unwrap().iter().any(|x| x.1 == "devcluster.restart"));
    lock(&w.d.over).poll = Some(Duration::from_millis(20));
    w.d.stop("root").unwrap();
}

#[test]
fn k3s_ending_after_the_machine_moved_to_another_network_is_expected_and_restarted_at_once_without_a_failure() {
    let (w, _route) = world_net("k3sdev-ipchange", GOOD_K3S, true);
    w.nodes_ready();
    w.d.assume_ip(Some("192.168.50.72"));
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running));
    let pid = child_pid(&w);
    // the stick is on another network: the address changed, and k3s exits itself
    w.d.assume_ip(Some("192.168.50.61"));
    unsafe { libc::kill(-(pid as i32), libc::SIGTERM) };
    assert!(wait_for(10, || w.actions().iter().any(|a| a == "devcluster.exited-ip-changed")), "{:?}", w.actions());
    assert!(wait_for(10, || try_pid(&w).is_some_and(|p| p != pid) && w.d.state() == Svc::Running));
    assert_eq!(w.d.installed().unwrap().restarts, 0, "a move is not a restart that counts");
    assert!(!w.actions().iter().any(|a| a == "devcluster.exited"));
    assert!(w.d.take_alerts().is_empty());
    // the same exit with the address unchanged is a failure as before
    let pid = child_pid(&w);
    unsafe { libc::kill(-(pid as i32), libc::SIGTERM) };
    assert!(wait_for(10, || w.actions().iter().any(|a| a == "devcluster.exited")), "{:?}", w.actions());
    w.d.stop("root").unwrap();
}

#[test]
fn exit_text_names_codes_signals_and_core_dumps() {
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(exit_text(&std::process::ExitStatus::from_raw(3 << 8)), "exit status 3");
    assert_eq!(exit_text(&std::process::ExitStatus::from_raw(9)), "signal 9 (SIGKILL)");
    assert_eq!(exit_text(&std::process::ExitStatus::from_raw(11 | 0x80)), "signal 11 (SIGSEGV), core dumped");
    assert_eq!(default_route_iface(&route_text(&format!("{ROUTE_LAN}{ROUTE_DEFAULT}"))).as_deref(), Some("eth0"));
}

// ------------------------------------------------------------------------------------------------ the clock gate

/// A clock the test moves by hand: `advance` moves both the wall and the monotonic time, `jump` only the wall time.
struct FakeClock {
    wall: std::sync::atomic::AtomicI64,
    mono_ms: AtomicU64,
    pending: AtomicBool,
    synced: AtomicBool,
}

impl FakeClock {
    fn new(pending: bool, synced: bool) -> Arc<FakeClock> {
        Arc::new(FakeClock { wall: std::sync::atomic::AtomicI64::new(1_704_067_320), mono_ms: AtomicU64::new(10_000), pending: AtomicBool::new(pending), synced: AtomicBool::new(synced) })
    }
    fn advance(&self, secs: u64) {
        self.mono_ms.fetch_add(secs * 1000, Ordering::SeqCst);
        self.wall.fetch_add(secs as i64, Ordering::SeqCst);
    }
    fn jump(&self, secs: i64) {
        self.wall.fetch_add(secs, Ordering::SeqCst);
    }
}

impl ClockSource for FakeClock {
    fn wall(&self) -> i64 {
        self.wall.load(Ordering::SeqCst)
    }
    fn mono(&self) -> Duration {
        Duration::from_millis(self.mono_ms.load(Ordering::SeqCst))
    }
    fn floor_pending(&self) -> bool {
        self.pending.load(Ordering::SeqCst)
    }
    fn synced(&self) -> bool {
        self.synced.load(Ordering::SeqCst)
    }
}

#[test]
fn the_clock_rules_are_pure_functions() {
    let lim = Duration::from_secs(120);
    assert!(!clock_ready(true, true, Duration::from_secs(500), lim), "never before the floor was applied");
    assert!(!clock_ready(false, false, Duration::from_secs(119), lim), "no time server yet and not long enough");
    assert!(clock_ready(false, true, Duration::from_secs(5), lim), "a time server answered");
    assert!(clock_ready(false, false, Duration::from_secs(120), lim), "offline: the floor is good enough after the limit");
    let m = Duration::from_secs;
    assert_eq!(clock_stepped(1000, m(50), 1010, m(60), 180), None, "time passing normally");
    assert_eq!(clock_stepped(1000, m(50), 1000 + 88_692_568, m(60), 180), Some(88_692_558), "the NTP jump");
    assert_eq!(clock_stepped(1000, m(50), 1010 - 3600, m(60), 180), Some(-3600), "a step back counts too");
    assert_eq!(clock_stepped(1000, m(50), 1100, m(60), 180), None, "a small step is ignored");
    assert!(log_shows_clock_trouble("E Failed to validate connection to cluster: CA cert validation failed: x"));
    assert!(log_shows_clock_trouble("tls: failed to verify certificate: x509: certificate has expired or is not yet valid"));
    assert!(!log_shows_clock_trouble("x509: certificate signed by unknown authority"));
}

#[test]
fn k3s_waits_for_the_floor_and_then_for_a_time_server_or_the_limit_and_the_wait_is_audited() {
    let (w, _route) = world_net("k3sdev-clockgate", GOOD_K3S, true);
    w.nodes_ready();
    let clock = FakeClock::new(true, false);
    w.d.assume_clock(clock.clone());
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.actions().iter().any(|a| a == "devcluster.waiting-for-clock")), "{:?}", w.actions());
    std::thread::sleep(Duration::from_millis(150));
    assert!(try_pid(&w).is_none() && w.d.state() == Svc::Starting, "nothing starts while the floor is pending");
    // the floor is applied, but no time server has answered and the machine is young
    clock.pending.store(false, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(150));
    assert!(try_pid(&w).is_none(), "still waiting for the time");
    // offline for two minutes: the floor is good enough
    clock.advance(120);
    assert!(wait_for(10, || w.d.state() == Svc::Running), "{:?}", w.d.state());
    assert!(try_pid(&w).is_some());
    assert!(w.d.log_tail(20).contains("no time server answered"), "{}", w.d.log_tail(20));
    w.d.stop("root").unwrap();
}

#[test]
fn a_time_server_answering_opens_the_gate_at_once_and_a_ready_clock_never_waits() {
    let (w, _route) = world_net("k3sdev-clocksynced", GOOD_K3S, true);
    w.nodes_ready();
    let clock = FakeClock::new(true, false);
    w.d.assume_clock(clock.clone());
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.actions().iter().any(|a| a == "devcluster.waiting-for-clock")));
    clock.pending.store(false, Ordering::SeqCst);
    clock.synced.store(true, Ordering::SeqCst);
    assert!(wait_for(10, || w.d.state() == Svc::Running), "{:?}", w.d.state());
    w.d.stop("root").unwrap();
    // a clock that is fine from the start: no waiting at all
    let (w2, _r) = world_net("k3sdev-clockfine", GOOD_K3S, true);
    w2.nodes_ready();
    w2.d.assume_clock(FakeClock::new(false, true));
    w2.d.start("root").unwrap();
    assert!(wait_for(10, || w2.d.state() == Svc::Running));
    assert!(!w2.actions().iter().any(|a| a == "devcluster.waiting-for-clock"));
    w2.d.stop("root").unwrap();
}

#[test]
fn stop_cancels_the_wait_for_the_clock() {
    let (w, _route) = world_net("k3sdev-clockstop", GOOD_K3S, true);
    let clock = FakeClock::new(true, false);
    w.d.assume_clock(clock);
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.actions().iter().any(|a| a == "devcluster.waiting-for-clock")));
    w.d.stop("root").unwrap();
    assert_eq!(w.d.state(), Svc::Stopped);
    assert!(try_pid(&w).is_none());
}

#[test]
fn a_clock_step_while_k3s_runs_restarts_it_once_without_a_failure_and_audits_it() {
    let (w, _route) = world_net("k3sdev-clockstep", GOOD_K3S, true);
    w.nodes_ready();
    let clock = FakeClock::new(false, true);
    w.d.assume_clock(clock.clone());
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running));
    let pid = child_pid(&w);
    // time passing normally changes nothing
    clock.advance(30);
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(child_pid(&w), pid);
    // the network time arrives: 2.8 years in one step
    clock.jump(88_692_568);
    assert!(wait_for(10, || w.actions().iter().any(|a| a == "devcluster.restart-clock-step")), "{:?}", w.actions());
    assert!(wait_for(10, || try_pid(&w).is_some_and(|p| p != pid) && w.d.state() == Svc::Running), "{:?}", w.d.state());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(w.actions().iter().filter(|a| *a == "devcluster.restart-clock-step").count(), 1, "one restart per step");
    assert_eq!(w.d.installed().unwrap().restarts, 0, "not counted as a failure");
    assert!(!w.actions().iter().any(|a| a == "devcluster.exited") && w.d.take_alerts().is_empty());
    w.d.stop("root").unwrap();
}

#[test]
fn a_k3s_that_ends_right_after_the_clock_moved_is_started_again_without_a_failure() {
    let (w, _route) = world_net("k3sdev-clockdied", GOOD_K3S, true);
    w.nodes_ready();
    let clock = FakeClock::new(false, true);
    w.d.assume_clock(clock.clone());
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running));
    let pid = child_pid(&w);
    // the jump and the exit are seen in the same look at k3s
    let guard = lock(&w.d.child);
    clock.jump(1_000_000);
    unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    std::thread::sleep(Duration::from_millis(100));
    drop(guard);
    assert!(wait_for(10, || w.actions().iter().any(|a| a == "devcluster.restart-clock-step")), "{:?}", w.actions());
    assert!(wait_for(10, || try_pid(&w).is_some_and(|p| p != pid) && w.d.state() == Svc::Running));
    assert!(!w.actions().iter().any(|a| a == "devcluster.exited"), "{:?}", w.actions());
    assert_eq!(w.d.installed().unwrap().restarts, 0);
    w.d.stop("root").unwrap();
}

/// A k3s that complains about certificates the way it does at the fake date.
const CERT_K3S: &str = "#!/bin/sh\ncase \"$1\" in\nserver)\n  kc=\n  while [ $# -gt 0 ]; do if [ \"$1\" = \"--write-kubeconfig\" ]; then kc=\"$2\"; fi; shift; done\n  echo \"level=error msg=Failed to validate connection to cluster: CA cert validation failed: x509: certificate has expired or is not yet valid\"\n  printf 'apiVersion: v1\\n' > \"$kc\"\n  exec sleep 600;;\nesac\n";

#[test]
fn certificate_trouble_in_the_first_minutes_restarts_k3s_once_when_the_clock_is_plausible() {
    let (w, _route) = world_net("k3sdev-certlog", CERT_K3S, true);
    w.nodes_ready();
    let clock = FakeClock::new(false, true);
    w.d.assume_clock(clock);
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.actions().iter().any(|a| a == "devcluster.restart-clock-cert")), "{:?}", w.actions());
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(w.actions().iter().filter(|a| *a == "devcluster.restart-clock-cert").count(), 1, "only once, even when the complaint repeats");
    assert_eq!(w.d.installed().unwrap().restarts, 0);
    assert!(!w.actions().iter().any(|a| a == "devcluster.exited"));
    w.d.stop("root").unwrap();
}

// ------------------------------------------------------------------------------------------------ more than one local cluster

/// A stand-in k3s that also records the arguments it was started with next to the kubeconfig.
const ARGS_K3S: &str = "#!/bin/sh\ncase \"$1\" in\nserver)\n  kc=\n  all=\"$*\"\n  while [ $# -gt 0 ]; do if [ \"$1\" = \"--write-kubeconfig\" ]; then kc=\"$2\"; fi; shift; done\n  echo \"$all\" > \"$kc.args\"\n  printf 'apiVersion: v1\\n' > \"$kc\"\n  exec sleep 600;;\nesac\n";

struct Extra {
    d: Arc<K3sDev>,
    audit: Arc<Mutex<Vec<(String, String, String)>>>,
}

impl Extra {
    fn actions(&self) -> Vec<String> {
        self.audit.lock().unwrap().iter().map(|a| a.1.clone()).collect()
    }
}

/// A second managed cluster next to `w`'s: same data partition and runner, its own instance, a stand-in program, the same fakes.
fn extra_of(w: &World, id: &str, slot: u16) -> Extra {
    let inst = Instance::extra(id, slot);
    let clis = Arc::new(CliManager::new(&w.dir.join("cli")));
    let d = K3sDev::new_instance(&w.dir.join("data"), clis, w.run.clone(), inst);
    let l = d.layout().clone();
    std::fs::create_dir_all(l.bin().parent().unwrap()).unwrap();
    std::fs::write(l.bin(), ARGS_K3S).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(l.bin(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    save_state(&l, &State { version: VERSION.into(), ..State::default() }).unwrap();
    let machine = lock(&w.d.machine).clone();
    d.assume(true, true, false, false, machine, Duration::from_millis(10), Duration::from_millis(20));
    d.assume_connected(None);
    d.assume_route_file(&w.dir.join("route"), Duration::from_millis(100));
    let audit: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(vec![]));
    let a2 = audit.clone();
    d.set_audit(Arc::new(move |u, a, x| a2.lock().unwrap().push((u.into(), a.into(), x.into()))));
    assert_eq!(d.state(), Svc::Stopped, "a folder with state.json is an installed cluster");
    Extra { d, audit }
}

#[test]
fn the_primary_instance_is_exactly_todays_cluster_and_an_extra_gets_its_own_ports_ranges_names_and_folder() {
    let l = Layout::new(Path::new("/d"));
    let p = Instance::primary();
    assert_eq!((p.id.as_str(), p.api_port, p.node_name.as_str(), p.pod_name.as_str(), p.cluster_cidr.as_str(), p.service_cidr.as_str()), ("local", 6443, "iyakki-dev", "iyakki-k3s", "10.42.0.0/16", "10.43.0.0/16"));
    assert_eq!(Layout::for_instance(Path::new("/d"), &p).root, l.root);
    let old: Vec<String> = ["server", "--data-dir", "/d/k3s-dev/data", "--write-kubeconfig", "/d/k3s-dev/kubeconfig", "--write-kubeconfig-mode", "600", "--disable", "traefik", "--disable", "servicelb", "--node-name", "iyakki-dev"].iter().map(|s| s.to_string()).collect();
    assert_eq!(server_args(&l), old, "the single-cluster command line did not change");
    assert!(!podman_create(&l, "img", &p).iter().any(|a| a.contains("cidr") || a.contains("listen-port")));
    // extras: nothing is shared with the primary or with each other
    let ins: Vec<Instance> = std::iter::once(p.clone()).chain((1..=4).map(|s| Instance::extra(&format!("c{s}"), s))).collect();
    for (i, a) in ins.iter().enumerate() {
        for b in ins.iter().skip(i + 1) {
            assert_ne!(a.dir_name, b.dir_name);
            assert_ne!(a.node_name, b.node_name);
            assert_ne!(a.pod_name, b.pod_name);
            assert_ne!(a.cluster_cidr, b.cluster_cidr);
            assert_ne!(a.service_cidr, b.service_cidr);
            let (pa, pb) = (a.all_ports(), b.all_ports());
            assert!(!pa.iter().any(|x| pb.contains(x)), "{} and {} share a port: {pa:?} {pb:?}", a.id, b.id);
        }
    }
    let e = Instance::extra("gpu2", 1);
    assert_eq!((e.api_port, e.node_name.as_str(), e.cluster_dns().as_str()), (6543, "iyakki-dev-gpu2", "10.45.0.10"));
    let args = server_args_for(&Layout::for_instance(Path::new("/d"), &e), &e);
    for want in ["--https-listen-port=6543", "--cluster-cidr=10.44.0.0/16", "--service-cidr=10.45.0.0/16", "--cluster-dns=10.45.0.10", "--node-name", "iyakki-dev-gpu2", "/d/k3s-dev-gpu2/data"] {
        assert!(args.iter().any(|a| a == want || a.contains(want)), "{want} in {args:?}");
    }
    let t = ports_for(&e);
    assert!(t[0].0 == "6543/tcp" && t.iter().any(|x| x.0 == "30000-32767/tcp"));
    assert!(valid_id("gpu2") && !valid_id("local") && !valid_id("a") && !valid_id("Bad") && !valid_id("-x"));
}

#[test]
fn two_local_clusters_start_and_stop_on_their_own_with_their_own_arguments_and_logs() {
    let (w, _route) = world_net("k3sdev-two", ARGS_K3S, true);
    w.nodes_ready();
    let x = extra_of(&w, "gpu2", 1);
    w.d.start("root").unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running), "{:?}", w.d.state());
    x.d.start("root").unwrap();
    assert!(wait_for(10, || x.d.state() == Svc::Running), "{:?}", x.d.state());
    let a1 = std::fs::read_to_string(format!("{}.args", w.d.layout().kubeconfig().display())).unwrap();
    let a2 = std::fs::read_to_string(format!("{}.args", x.d.layout().kubeconfig().display())).unwrap();
    assert!(!a1.contains("--https-listen-port") && a1.contains("--node-name iyakki-dev"), "{a1}");
    assert!(a2.contains("--https-listen-port=6543") && a2.contains("--node-name iyakki-dev-gpu2") && a2.contains("k3s-dev-gpu2/data"), "{a2}");
    assert_ne!(w.d.layout().log(), x.d.layout().log());
    assert_ne!(w.d.layout().state(), x.d.layout().state());
    assert!(w.d.layout().log().is_file() && x.d.layout().log().is_file());
    assert_ne!(lock(&w.d.child).as_ref().map(|c| c.id()), lock(&x.d.child).as_ref().map(|c| c.id()));
    // stopping the extra leaves the primary running, and the other way round
    x.d.stop("root").unwrap();
    assert_eq!(x.d.state(), Svc::Stopped);
    assert_eq!(w.d.state(), Svc::Running);
    assert!(x.actions().iter().any(|a| a == "devcluster.stop") && !w.actions().iter().any(|a| a == "devcluster.stop"));
    x.d.start("root").unwrap();
    assert!(wait_for(10, || x.d.state() == Svc::Running));
    w.d.stop("root").unwrap();
    assert_eq!((w.d.state(), x.d.state()), (Svc::Stopped, Svc::Running));
    x.d.stop("root").unwrap();
}

#[test]
fn the_clock_gate_applies_to_each_instance_on_its_own() {
    let (w, _route) = world_net("k3sdev-twoclock", ARGS_K3S, true);
    w.nodes_ready();
    let x = extra_of(&w, "gpu2", 1);
    let clock = FakeClock::new(true, false);
    x.d.assume_clock(clock.clone());
    w.d.assume_clock(FakeClock::new(false, true));
    w.d.start("root").unwrap();
    x.d.start("root").unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running), "the first one has a good clock and does not wait");
    assert!(wait_for(10, || x.actions().iter().any(|a| a == "devcluster.waiting-for-clock")), "{:?}", x.actions());
    assert!(!w.actions().iter().any(|a| a == "devcluster.waiting-for-clock"));
    assert!(lock(&x.d.child).is_none(), "the second one is not started before its clock is ready");
    clock.pending.store(false, Ordering::SeqCst);
    clock.synced.store(true, Ordering::SeqCst);
    assert!(wait_for(10, || x.d.state() == Svc::Running), "{:?}", x.d.state());
    x.d.stop("root").unwrap();
    w.d.stop("root").unwrap();
}

#[test]
fn the_watchdog_of_an_extra_instance_restarts_only_that_instance() {
    let (w, _route) = world_net("k3sdev-twowatch", ARGS_K3S, true);
    w.nodes_ready();
    let x = extra_of(&w, "gpu2", 1);
    w.d.start("root").unwrap();
    x.d.start("root").unwrap();
    assert!(wait_for(10, || w.d.state() == Svc::Running && x.d.state() == Svc::Running));
    let (p1, p2) = (lock(&w.d.child).as_ref().unwrap().id(), lock(&x.d.child).as_ref().unwrap().id());
    lock(&x.d.over).poll = Some(Duration::from_secs(3600));
    std::thread::sleep(Duration::from_millis(300));
    unsafe { libc::kill(-(p2 as i32), libc::SIGKILL) };
    std::thread::sleep(Duration::from_millis(200));
    let gen = x.d.gen.load(Ordering::SeqCst);
    assert!(!x.d.watchdog_step(gen, Backend::Native, Duration::from_secs(3600)), "a fresh heartbeat: leave it alone");
    assert!(x.d.watchdog_step(gen, Backend::Native, Duration::ZERO), "stale: take over");
    assert!(x.actions().iter().any(|a| a == "devcluster.supervisor-stalled"), "{:?}", x.actions());
    assert_eq!(x.d.installed().unwrap().restarts, 1);
    assert_ne!(lock(&x.d.child).as_ref().unwrap().id(), p2);
    assert_eq!(lock(&w.d.child).as_ref().unwrap().id(), p1, "the other cluster was not touched");
    assert_eq!(w.d.installed().unwrap().restarts, 0);
    lock(&x.d.over).poll = Some(Duration::from_millis(20));
    x.d.stop("root").unwrap();
    w.d.stop("root").unwrap();
}

#[test]
fn another_managed_cluster_is_not_a_cluster_found_elsewhere_but_a_foreign_one_still_is() {
    let w = world("k3sdev-found");
    let proc = lock(&w.d.machine).proc_dir.clone();
    std::fs::create_dir_all(proc.join("4242")).unwrap();
    std::fs::write(proc.join("4242/comm"), "k3s-server\n").unwrap();
    let x = extra_of(&w, "gpu2", 1);
    assert!(w.d.found_elsewhere().iter().any(|f| f.contains("k3s-server")), "the first cluster still refuses next to a foreign k3s");
    assert!(!x.d.found_elsewhere().iter().any(|f| f.contains("k3s-server")), "an extra cluster expects the first one's processes");
}

#[test]
fn an_extra_instance_asks_for_its_own_node_name_to_reset_and_uninstall() {
    let w = world("k3sdev-twotype");
    let x = extra_of(&w, "gpu2", 1);
    let e = x.d.reset("iyakki-dev", "root").unwrap_err();
    assert!(e.contains("iyakki-dev-gpu2"), "{e}");
    x.d.reset("iyakki-dev-gpu2", "root").unwrap();
    assert!(x.d.uninstall("iyakki-dev", "root").is_err());
    x.d.uninstall("iyakki-dev-gpu2", "root").unwrap();
    assert_eq!(x.d.state(), Svc::NotInstalled);
    assert!(!x.d.layout().root.exists());
    assert!(w.d.layout().root.parent() == x.d.layout().root.parent() && w.d.layout().root != x.d.layout().root);
}
