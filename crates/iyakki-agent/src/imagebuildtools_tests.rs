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
    fn set_builder_image(&self, image: Option<&str>) -> Result<(), String> {
        self.ran.lock().unwrap().push(format!("set:{}", image.unwrap_or("none")));
        Ok(())
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
    assert_eq!(d.len(), 3);
    let d: Vec<ToolDef> = d.into_iter().filter(|x| x.spec.name == TOOL).collect();
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

#[test]
fn the_builder_image_is_shown_for_free_and_set_only_by_an_administrator_with_a_real_digest_after_asking() {
    let (t, f) = rig(true, false, true, "builder");
    let d = t.defs();
    let risk = |n: &str| d.iter().find(|x| x.spec.name == n).map(|x| matches!(x.risk, Risk::Safe));
    assert_eq!((risk(STATUS_TOOL), risk(SET_TOOL)), (Some(true), Some(false)));
    assert!(!t.approved_always(SET_TOOL) && !t.pre_approved(SET_TOOL) && t.grant_blocker(SET_TOOL, &json!({})).is_some());
    assert!(t.call(STATUS_TOOL, &json!({})).unwrap().contains("No builder image"));
    // only a pinned image passes; a tag, all zeros, a space and no admin do not
    let good = format!("quay.io/buildah/stable@{DIGEST}");
    for bad in ["quay.io/buildah/stable:latest".to_string(), format!("x@sha256:{}", "0".repeat(64)), format!("x y@{DIGEST}"), format!("@{DIGEST}"), "x@sha256:abc".to_string()] {
        assert!(t.refusal(SET_TOOL, &json!({"image": bad})).is_some(), "{bad}");
        assert!(t.call(SET_TOOL, &json!({"image": bad})).is_err(), "{bad}");
    }
    assert!(f.ran.lock().unwrap().is_empty(), "nothing was written for a bad image");
    assert_eq!(t.describe(SET_TOOL, &json!({"image": good})), format!("Use {good} as the image builder"));
    assert!(t.call(SET_TOOL, &json!({"image": good})).unwrap().contains(&good));
    assert_eq!(f.ran.lock().unwrap().clone(), vec![format!("set:{good}")]);
    assert!(t.call(SET_TOOL, &json!({})).unwrap().contains("cleared"));
    let (user, _) = rig(false, false, true, "builder2");
    assert!(user.call(SET_TOOL, &json!({"image": good})).unwrap_err().contains("administrator"));
}

#[test]
fn the_real_backend_keeps_the_choice_in_a_file_and_only_a_real_digest_counts() {
    use iyakki_shared::imagebuildtools::builder_pinned;
    assert!(builder_pinned(&format!("a/b@{DIGEST}")));
    assert!(!builder_pinned("a/b@sha256:") && !builder_pinned("a/b") && !builder_pinned(&format!("a/b@sha256:{}", DIGEST[7..].to_uppercase())));
    // what this build ships is empty until the owner pins one
    assert!(!builder_pinned(BUILDER_IMAGE));
}
