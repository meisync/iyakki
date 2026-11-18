use super::*;
use crate::linuxpod_tests::{gzip_stored, root, tar, tmp, Keep, Spy};
use std::sync::Mutex;

struct Inner;
impl Tools for Inner {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        Err(format!("inner got {name}"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.into()
    }
}

struct Fake {
    admin: bool,
    running: bool,
    dir: PathBuf,
    kube: Arc<Spy>,
    imp: Arc<Keep>,
    audit: Mutex<Vec<String>>,
}

impl Fake {
    fn new(tag: &str, admin: bool, running: bool, spy: Spy) -> Arc<Fake> {
        Arc::new(Fake { admin, running, dir: tmp(tag), kube: Arc::new(spy), imp: Arc::new(Keep::default()), audit: Mutex::new(vec![]) })
    }
}

struct KubeRef(Arc<Spy>);
impl Kube for KubeRef {
    fn run(&self, a: &[String]) -> crate::wsexec::Out {
        self.0.run(a)
    }
}
struct ImpRef(Arc<Keep>);
impl Importer for ImpRef {
    fn import(&self, f: &std::path::Path) -> Result<(), String> {
        self.0.import(f)
    }
}

impl Backend for Fake {
    fn is_admin(&self) -> bool {
        self.admin
    }
    fn user(&self) -> String {
        "Admin".into()
    }
    fn archive(&self, path: &str) -> Result<Vec<u8>, String> {
        if path.contains("..") || path.starts_with('/') {
            return Err("the archive has to be a file inside your own home folder".into());
        }
        std::fs::read(self.dir.join(path)).map_err(|_| format!("{path} is not a file"))
    }
    fn dev_cluster_running(&self) -> bool {
        self.running
    }
    fn kube(&self) -> Arc<dyn Kube> {
        Arc::new(KubeRef(self.kube.clone()))
    }
    fn importer(&self) -> Arc<dyn Importer> {
        Arc::new(ImpRef(self.imp.clone()))
    }
    fn work_dir(&self) -> PathBuf {
        self.dir.join("work")
    }
    fn arch(&self) -> String {
        "amd64".into()
    }
    fn audit(&self, a: &str, d: &str) {
        self.audit.lock().unwrap().push(format!("{a} {d}"));
    }
}

fn tools(f: &Arc<Fake>) -> LinuxPodTools {
    LinuxPodTools::new(Arc::new(Inner), f.clone())
}

fn running() -> Spy {
    Spy { phases: Mutex::new(vec!["Running"]), has_pod: true, pods: "pod/linuxapp-admin-dev", ..Default::default() }
}

#[test]
fn the_list_is_free_and_starting_and_stopping_ask_every_time_with_no_grant() {
    let f = Fake::new("defs", true, true, running());
    let t = tools(&f);
    let d = t.defs();
    let risk = |n: &str| d.iter().find(|x| x.spec.name == n).unwrap().risk;
    assert_eq!((risk("linuxapp_pods"), risk("linuxapp_pod_start"), risk("linuxapp_pod_stop")), (Risk::Safe, Risk::Ask, Risk::Ask));
    for n in ["linuxapp_pod_start", "linuxapp_pod_stop"] {
        assert!(t.grant_blocker(n, &json!({})).is_some() && !t.approved_always(n) && !t.pre_approved(n), "{n}");
    }
    assert_eq!(t.call("something_else", &json!({})).unwrap_err(), "inner got something_else");
    assert_eq!(t.call("linuxapp_pods", &json!({})).unwrap(), "linuxapp-admin-dev");
}

#[test]
fn a_good_ask_converts_imports_and_starts_once_and_the_answer_names_the_exec_command() {
    let f = Fake::new("good", true, true, running());
    std::fs::write(f.dir.join("root.tar.gz"), gzip_stored(&root())).unwrap();
    let t = tools(&f);
    let a = json!({"name": "dev", "archive": "root.tar.gz"});
    let card = t.preview("linuxapp_pod_start", &a).unwrap();
    assert!(card.contains("4 files") && card.contains("linuxapp-admin-dev") && card.contains("set-user-id") && card.contains("dev/null") && card.contains("never pulls"), "{card}");
    assert!(f.kube.calls.lock().unwrap().is_empty() && f.imp.seen.lock().unwrap().is_empty(), "the card changes nothing");
    assert!(t.refusal("linuxapp_pod_start", &a).is_none());
    let out = t.call("linuxapp_pod_start", &a).unwrap();
    assert!(out.contains("kubectl exec -n iyakki-apps linuxapp-admin-dev -- /bin/sh") && out.contains("iyakki.local/linuxapp-admin-dev:"), "{out}");
    assert_eq!(f.imp.seen.lock().unwrap().len(), 1);
    assert_eq!(f.kube.applied.lock().unwrap().len(), 1);
    assert!(f.audit.lock().unwrap().iter().any(|l| l.starts_with("linuxapp.pod_start linuxapp-admin-dev image=iyakki.local/")), "{:?}", f.audit.lock().unwrap());
    // stopping asks too and removes the pod
    assert!(t.call("linuxapp_pod_stop", &json!({"name": "dev"})).unwrap().contains("linuxapp-admin-dev"));
    assert!(f.audit.lock().unwrap().iter().any(|l| l == "linuxapp.pod_stop linuxapp-admin-dev"));
    assert_eq!(t.describe("linuxapp_pod_start", &a), "Start the Linux app “dev” as a pod in the dev cluster from root.tar.gz");
}

#[test]
fn everything_wrong_is_refused_in_words_before_anything_is_imported_or_applied() {
    let ok = json!({"name": "dev", "archive": "root.tar.gz"});
    // not an administrator, no dev cluster
    for (tag, admin, running_, want) in [("noadmin", false, true, "administrator"), ("nocluster", true, false, "not running")] {
        let f = Fake::new(tag, admin, running_, running());
        std::fs::write(f.dir.join("root.tar.gz"), gzip_stored(&root())).unwrap();
        let t = tools(&f);
        assert!(t.refusal("linuxapp_pod_start", &ok).unwrap().contains(want));
        assert!(t.call("linuxapp_pod_start", &ok).is_err() && t.call("linuxapp_pod_stop", &json!({"name": "dev"})).is_err());
        assert!(f.kube.calls.lock().unwrap().is_empty() && f.imp.seen.lock().unwrap().is_empty());
    }
    let f = Fake::new("bad", true, true, running());
    std::fs::write(f.dir.join("root.tar.gz"), gzip_stored(&root())).unwrap();
    std::fs::write(f.dir.join("notes.txt"), b"hello").unwrap();
    std::fs::write(f.dir.join("unsafe.tar"), tar(&[("../x", b'0', "x", 0o644)])).unwrap();
    let t = tools(&f);
    for (args, want) in [
        (json!({"archive": "root.tar.gz"}), "name"),
        (json!({"name": "Dev", "archive": "root.tar.gz"}), "lower-case"),
        (json!({"name": "dev"}), "archive"),
        (json!({"name": "dev", "archive": "../etc/passwd"}), "inside your own home"),
        (json!({"name": "dev", "archive": "/etc/passwd"}), "inside your own home"),
        (json!({"name": "dev", "archive": "missing.tar.gz"}), "not a file"),
        (json!({"name": "dev", "archive": "root.tar.gz", "command": "sh"}), "short list"),
        (json!({"name": "dev", "archive": "root.tar.gz", "command": []}), "short list"),
    ] {
        let e = t.call("linuxapp_pod_start", &args).unwrap_err();
        assert!(e.contains(want), "{args}: {e}");
    }
    assert!(t.call("linuxapp_pod_start", &json!({"name": "dev", "archive": "notes.txt"})).unwrap_err().contains("neither"));
    assert!(t.call("linuxapp_pod_start", &json!({"name": "dev", "archive": "unsafe.tar"})).is_err());
    assert!(f.kube.calls.lock().unwrap().is_empty() && f.imp.seen.lock().unwrap().is_empty(), "nothing was imported or applied");
    assert!(f.audit.lock().unwrap().iter().all(|l| l.contains("failed")), "{:?}", f.audit.lock().unwrap());
}

#[test]
fn the_real_backend_reads_an_archive_only_inside_the_persons_own_home_and_within_the_size_limit() {
    struct Dead;
    impl iyakki_proxy::Upstream for Dead {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }
    let d = tmp("real");
    let svc = crate::services::Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let be = RealBackend { svc: svc.clone(), policy: iyakki_sec::policy::Role::Admin.template(), user: "podann".into(), audit: Arc::new(|_, _| {}) };
    assert!(be.is_admin());
    let home = crate::scope::home_root_for_writes("podann", svc.dir());
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("root.tar.gz"), gzip_stored(&root())).unwrap();
    assert_eq!(be.archive("root.tar.gz").unwrap().len(), gzip_stored(&root()).len());
    assert!(be.archive("../outside").unwrap_err().contains(".."));
    assert!(be.archive("nope.tar.gz").unwrap_err().contains("not a file"));
    std::fs::write(d.join("elsewhere.tar.gz"), b"x").unwrap();
    assert!(be.archive(&d.join("elsewhere.tar.gz").display().to_string()).unwrap_err().contains("inside your own home"));
    // a link out of the home does not get an archive out either
    let _ = std::fs::remove_file(home.join("link.tar.gz"));
    std::os::unix::fs::symlink(d.join("elsewhere.tar.gz"), home.join("link.tar.gz")).unwrap();
    assert!(be.archive("link.tar.gz").unwrap_err().contains("inside your own home"));
    // a folder is not an archive
    std::fs::create_dir_all(home.join("dir")).unwrap();
    assert!(be.archive("dir").is_err());
    assert!(!be.dev_cluster_running(), "no dev cluster is installed in this test");
    assert_eq!(be.arch(), if cfg!(target_arch = "aarch64") { "arm64" } else { "amd64" });
    assert!(be.work_dir().starts_with(svc.dir()));
}
