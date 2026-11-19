use std::sync::Mutex;

use super::*;
use crate::linuxpod_tests::{gzip_stored, root};
use crate::ocipush::tests::FakeRegistry;

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
    archive: Result<Vec<u8>, String>,
    reg: Arc<FakeRegistry>,
    apps: Mutex<Vec<(String, String)>>,
    audit: Mutex<Vec<String>>,
}

impl Backend for Fake {
    fn is_admin(&self) -> bool {
        self.admin
    }
    fn archive(&self, path: &str) -> Result<Vec<u8>, String> {
        if path.contains("..") { Err("the path may not contain ..".into()) } else { self.archive.clone() }
    }
    fn dev_cluster_running(&self) -> bool {
        self.running
    }
    fn registry(&self) -> String {
        "localhost:30500".into()
    }
    fn http(&self) -> Arc<dyn RegistryHttp> {
        self.reg.clone()
    }
    fn arch(&self) -> String {
        "amd64".into()
    }
    fn write_app(&self, id: &str, text: &str) -> Result<(), String> {
        self.apps.lock().unwrap().push((id.into(), text.into()));
        Ok(())
    }
    fn audit(&self, action: &str, detail: &str) {
        self.audit.lock().unwrap().push(format!("{action} {detail}"));
    }
}

fn rig(admin: bool, running: bool) -> (LinuxAppImageTools, Arc<Fake>) {
    let f = Arc::new(Fake { admin, running, archive: Ok(gzip_stored(&root())), reg: Arc::new(FakeRegistry::default()), apps: Mutex::new(vec![]), audit: Mutex::new(vec![]) });
    (LinuxAppImageTools::new(Arc::new(Inner), f.clone()), f)
}

fn args() -> Value {
    json!({"archive": "root.tar.gz", "name": "My Notes", "port": 8080, "licence": "MIT", "command": ["/usr/bin/notes", "--serve"]})
}

#[test]
fn the_tool_asks_every_time_is_for_administrators_and_names_what_goes_into_the_image() {
    let (t, _) = rig(true, true);
    let d = t.defs().into_iter().find(|d| d.spec.name == TOOL).unwrap();
    assert_eq!(d.risk, Risk::Ask);
    assert!(!t.pre_approved(TOOL) && !t.approved_always(TOOL) && t.grant_blocker(TOOL, &args()).is_some());
    let card = t.preview(TOOL, &args()).unwrap();
    assert!(card.contains("My Notes") && card.contains("/usr/bin/notes --serve") && card.contains("port 8080") && card.contains("MIT") && card.contains("my-notes:1"), "{card}");
    assert!(card.contains("files"), "{card}");
    let (user, _) = rig(false, true);
    assert!(user.refusal(TOOL, &args()).unwrap().contains("administrator"));
    let (off, _) = rig(true, false);
    assert!(off.refusal(TOOL, &args()).unwrap().contains("dev cluster is not running"));
}

#[test]
fn a_made_up_archive_becomes_an_image_in_the_registry_and_an_image_app_with_the_pinned_digest() {
    let (t, f) = rig(true, true);
    let said = t.call(TOOL, &args()).unwrap();
    assert!(said.contains("localhost:30500/my-notes:1@sha256:") && said.contains("Image apps"), "{said}");
    let m = f.reg.manifests.lock().unwrap();
    assert!(m.contains_key("my-notes:1"), "the manifest is in the registry: {:?}", m.keys().collect::<Vec<_>>());
    assert_eq!(f.reg.blobs.lock().unwrap().len(), 2, "the layer and the config");
    let apps = f.apps.lock().unwrap();
    assert_eq!(apps.len(), 1);
    let v: Value = serde_json::from_str(&apps[0].1).unwrap();
    assert_eq!(v["id"], "my-notes");
    assert!(v["image"].as_str().unwrap().contains("@sha256:") && v["licence"] == "MIT" && v["ports"][0]["port"] == 8080, "{v}");
    assert!(crate::imageapps::parse(&apps[0].1).is_ok(), "the image apps reader accepts the file");
    let log = f.audit.lock().unwrap().join("|");
    assert!(log.contains("linuxapp.image my-notes") && log.contains("linuxapp.image.done my-notes sha256:"), "{log}");
}

#[test]
fn every_missing_or_unsafe_argument_is_refused_before_anything_is_pushed() {
    let (t, f) = rig(true, true);
    for (what, patch) in [("licence", json!({"licence": ""})), ("port below 1024", json!({"port": 80})), ("command without a full path", json!({"command": ["notes"]})), ("empty command", json!({"command": []})), ("tag with a slash", json!({"tag": "a/b"})), ("path with ..", json!({"archive": "../x.tar.gz"})), ("name without letters", json!({"name": "---"}))] {
        let mut a = args();
        for (k, v) in patch.as_object().unwrap() {
            a[k] = v.clone();
        }
        assert!(t.call(TOOL, &a).is_err(), "{what} must be refused");
    }
    assert!(f.reg.manifests.lock().unwrap().is_empty() && f.apps.lock().unwrap().is_empty(), "nothing was pushed or written");
    // a damaged archive and a registry that is down
    let bad = Arc::new(Fake { archive: Ok(vec![1, 2, 3]), ..Fake { admin: true, running: true, archive: Ok(vec![]), reg: Arc::new(FakeRegistry::default()), apps: Mutex::new(vec![]), audit: Mutex::new(vec![]) } });
    assert!(LinuxAppImageTools::new(Arc::new(Inner), bad.clone()).call(TOOL, &args()).is_err());
    let (t2, f2) = rig(true, true);
    *f2.reg.down.lock().unwrap() = true;
    let e = t2.call(TOOL, &args()).unwrap_err();
    assert!(e.contains("pushing the image to the dev registry failed"), "{e}");
    assert!(f2.apps.lock().unwrap().is_empty(), "no app file when the push failed");
}
