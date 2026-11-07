//! The object storage chat tools with a fake backend: approval behaviour, names only, the data kept unless asked.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::objectstoragetools::*;
use crate::quickinstall::objstore::{Connection, Engine};
use crate::quickinstall::objstore::{RUSTFS, SEAWEEDFS};

const AK: &str = "AKIAFAKE0000EXAMPLE1";
const SK: &str = "fakeSecretKeyForTestsOnly0123456789ABCDEF";

#[derive(Default)]
struct Fake {
    installed: Mutex<Vec<&'static str>>,
    log: Mutex<Vec<String>>,
}
impl Backend for Fake {
    fn installed(&self, e: &Engine) -> Option<String> {
        self.installed.lock().unwrap().contains(&e.name).then(|| "http://x".to_string())
    }
    fn vault_names(&self) -> Vec<String> {
        vec!["svc.seaweedfs.access-key".into(), "svc.seaweedfs.secret-key".into(), "s3.seaweedfs".into()]
    }
    fn record(&self, e: &Engine) -> Option<String> {
        Connection::of(e).to_text().ok()
    }
    fn install(&self, e: &Engine) -> Result<String, String> {
        self.log.lock().unwrap().push(format!("install {}", e.name));
        Ok(format!("started {}", e.title))
    }
    fn remove(&self, e: &Engine, d: bool) -> Result<String, String> {
        self.log.lock().unwrap().push(format!("remove {} {d}", e.name));
        Ok("removed".into())
    }
}

fn rig() -> (ObjectStorageTools, Arc<Fake>) {
    let f = Arc::new(Fake::default());
    (ObjectStorageTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn status_is_free_and_writes_ask() {
    let (t, _) = rig();
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk("object_storage_status"), Some(Risk::Safe));
    assert_eq!(risk("object_storage_install"), Some(Risk::Ask));
    assert_eq!(risk("object_storage_remove"), Some(Risk::Ask));
    for n in WRITE_TOOLS {
        assert!(!t.pre_approved(n) && !t.approved_always(n));
        assert!(t.grant_blocker(n, &json!({"engine": "rustfs"})).is_some(), "asks every time, never remembered");
        assert!(crate::agent::hard_ask(n, &json!({})).is_some());
    }
}

#[test]
fn status_names_the_vault_entries_and_never_a_key() {
    let (t, f) = rig();
    let none = t.call("object_storage_status", &json!({})).unwrap();
    assert!(none.contains("SeaweedFS: not installed") && none.contains("RustFS: not installed"));
    f.installed.lock().unwrap().push("seaweedfs");
    let s = t.call("object_storage_status", &json!({})).unwrap();
    assert!(s.contains("svc.seaweedfs.access-key (in the vault)") && s.contains("s3.seaweedfs (in the vault)") && s.contains("bucket iyakki") && s.contains("inside the cluster only"), "{s}");
    assert!(!s.contains(AK) && !s.contains(SK));
}

#[test]
fn the_cards_say_what_happens_and_remove_says_whether_data_is_deleted() {
    let (t, _) = rig();
    let keep = t.preview("object_storage_remove", &json!({"engine": "seaweedfs"})).unwrap();
    assert!(keep.contains("kept") && !keep.contains("DELETE"), "{keep}");
    let del = t.preview("object_storage_remove", &json!({"engine": "rustfs", "delete_data": true})).unwrap();
    assert!(del.contains("DELETE ITS DATA") && del.contains("cannot be undone"), "{del}");
    let inst = t.preview("object_storage_install", &json!({"engine": "rustfs"})).unwrap();
    assert!(inst.contains("RustFS") && inst.contains("vault") && inst.contains("ClusterIP"), "{inst}");
    assert!(t.describe("object_storage_install", &json!({"engine": "seaweedfs"})).starts_with("Install SeaweedFS"));
}

#[test]
fn a_call_with_an_unknown_store_or_key_text_is_refused_without_echo() {
    let (t, f) = rig();
    assert!(t.refusal("object_storage_install", &json!({"engine": "minio"})).unwrap().contains("seaweedfs or rustfs"));
    assert!(t.refusal("object_storage_install", &json!({})).is_some());
    for key in [AK, SK] {
        let r = t.refusal("object_storage_remove", &json!({"engine": key})).unwrap();
        assert!(!r.contains(key) && r.contains("never send a key"), "{r}");
        assert!(t.call("object_storage_install", &json!({"engine": key})).is_err());
    }
    assert!(f.log.lock().unwrap().is_empty(), "nothing ran");
    assert!(t.refusal("object_storage_install", &json!({"engine": "rustfs"})).is_none());
}

#[test]
fn install_and_remove_reach_the_backend_with_the_default_of_keeping_data() {
    let (t, f) = rig();
    t.call("object_storage_install", &json!({"engine": "seaweedfs"})).unwrap();
    t.call("object_storage_remove", &json!({"engine": "rustfs"})).unwrap();
    t.call("object_storage_remove", &json!({"engine": "seaweedfs", "delete_data": true})).unwrap();
    assert_eq!(*f.log.lock().unwrap(), ["install seaweedfs", "remove rustfs false", "remove seaweedfs true"]);
    let _ = (SEAWEEDFS.name, RUSTFS.name);
}
