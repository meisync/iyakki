use std::sync::Mutex;

use super::*;

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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
    builder: Option<String>,
    running: bool,
    dir: PathBuf,
    ran: Mutex<Vec<String>>,
}
impl Backend for Fake {
    fn is_admin(&self) -> bool {
        self.admin
    }
    fn folder(&self, dir: &str) -> Result<PathBuf, String> {
        if dir.contains("..") { Err("the folder may not contain ..".into()) } else { Ok(self.dir.clone()) }
    }
    fn builder_image(&self) -> Option<String> {
        self.builder.clone()
    }
    fn registry(&self) -> String {
        "localhost:30500".into()
    }
    fn dev_cluster_running(&self) -> bool {
        self.running
    }
    fn run(&self, spec: &BuildSpec, ctx: &ib::Context, app_id: &str, title: &str, port: u16, licence: &str) -> Result<String, String> {
        self.ran.lock().unwrap().push(format!("{app_id}|{title}|{port}|{licence}|{}|{}", spec.repo, ctx.files));
        Ok("Built.".into())
    }
}

fn rig(admin: bool, builder: bool, running: bool, tag: &str) -> (ImageBuildTools, Arc<Fake>) {
    let dir = std::env::temp_dir().join(format!("iyakki-ibtools-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Containerfile"), "FROM scratch\n").unwrap();
    std::fs::write(dir.join(".env"), "T=1").unwrap();
    let f = Arc::new(Fake { admin, builder: builder.then(|| format!("quay.io/buildah/stable@{DIGEST}")), running, dir, ran: Mutex::new(vec![]) });
    (ImageBuildTools::new(Arc::new(Inner), f.clone()), f)
}

fn args() -> Value {
    json!({"dir": "projects/notes", "name": "My Notes", "port": 8080, "licence": "MIT"})
}

#[test]
fn the_tool_asks_every_time_and_a_card_lists_the_files_and_what_is_left_out() {
    let (t, _) = rig(true, true, true, "card");
    let d = t.defs();
    assert_eq!(d.len(), 1);
    assert!(matches!(d[0].risk, Risk::Ask) && !t.approved_always(TOOL) && !t.pre_approved(TOOL) && t.grant_blocker(TOOL, &args()).is_some());
    let card = t.preview(TOOL, &args()).unwrap();
    for want in ["“My Notes”", "1 file(s)", "port 8080", "licence MIT", "quay.io/buildah/stable@sha256:", "localhost:30500/my-notes:1", ".env (looks like a secret)", "15 minutes"] {
        assert!(card.contains(want), "{want}\n{card}");
    }
    assert_eq!(t.call("other", &json!({})).unwrap_err(), "inner got other");
}

#[test]
fn it_refuses_in_plain_words_until_everything_needed_is_there_and_builds_nothing_then() {
    let a = args();
    let (t, f) = rig(false, true, true, "r1");
    assert!(t.call(TOOL, &a).unwrap_err().contains("administrator"));
    let (t, _) = rig(true, false, true, "r2");
    assert!(t.call(TOOL, &a).unwrap_err().contains("no builder image is set up"));
    let (t, _) = rig(true, true, false, "r3");
    assert!(t.call(TOOL, &a).unwrap_err().contains("dev cluster is not running"));
    let (t, f2) = rig(true, true, true, "r4");
    let mut b = a.clone();
    b["licence"] = json!("");
    assert!(t.call(TOOL, &b).unwrap_err().contains("licence"));
    let mut b = a.clone();
    b["port"] = json!(80);
    assert!(t.call(TOOL, &b).unwrap_err().contains("1024"));
    let mut b = a.clone();
    b["dir"] = json!("../etc");
    assert!(t.call(TOOL, &b).unwrap_err().contains(".."));
    let mut b = a.clone();
    b["dockerfile"] = json!("Dockerfile");
    assert!(t.call(TOOL, &b).unwrap_err().contains("no Dockerfile"));
    let mut b = a.clone();
    b["name"] = json!("!!!");
    assert!(t.call(TOOL, &b).unwrap_err().contains("letters or digits"));
    assert!(f.ran.lock().unwrap().is_empty() && f2.ran.lock().unwrap().is_empty(), "nothing was built");
    assert!(t.refusal(TOOL, &b).is_some());
}

#[test]
fn a_good_ask_runs_the_build_once_with_the_checked_values() {
    let (t, f) = rig(true, true, true, "ok");
    assert_eq!(t.call(TOOL, &args()).unwrap(), "Built.");
    assert_eq!(*f.ran.lock().unwrap(), vec!["my-notes|My Notes|8080|MIT|my-notes|1".to_string()]);
    assert!(t.refusal(TOOL, &args()).is_none());
}
