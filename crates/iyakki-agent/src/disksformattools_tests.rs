//! The format and resize chat tools with a fake backend: the plan is free, format and resize ask every time and never get a standing
//! allow, the assistant has no argument that could carry the typed confirmation, and the request file holds the request only.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::disksformattools::*;
use crate::disks_part::FormatFs;

struct Fake {
    asked: Mutex<Vec<Req>>,
}

impl Backend for Fake {
    fn plan(&self, req: &Req) -> Result<String, String> {
        if req.part() == "sda1" {
            return Err("sda1 is on the drive that holds the running system. It is never changed.".into());
        }
        Ok(format!("Everything on /dev/{} will be erased.", req.part()))
    }
    fn request(&self, req: &Req) -> Result<String, String> {
        self.plan(req)?;
        self.asked.lock().unwrap().push(req.clone());
        Ok("The review is waiting in the DISKS tile.".into())
    }
}

struct Base;
impl Tools for Base {
    fn defs(&self) -> Vec<crate::agent::ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &serde_json::Value) -> Result<String, String> {
        Err(format!("no tool {name}"))
    }
    fn describe(&self, name: &str, _: &serde_json::Value) -> String {
        name.into()
    }
}

fn tools() -> (DisksFormatTools, Arc<Fake>) {
    let f = Arc::new(Fake { asked: Mutex::new(vec![]) });
    (DisksFormatTools::new(Arc::new(Base), f.clone()), f)
}

#[test]
fn the_plan_is_free_but_format_and_resize_ask_every_time_with_no_standing_allow() {
    let (t, _) = tools();
    let risk = |n: &str| t.defs().into_iter().find(|d| d.spec.name == n).unwrap().risk;
    assert_eq!(risk(PLAN_TOOL), Risk::Safe);
    assert_eq!(risk(FORMAT_TOOL), Risk::Ask);
    assert_eq!(risk(RESIZE_TOOL), Risk::Ask);
    for n in [FORMAT_TOOL, RESIZE_TOOL] {
        assert!(!t.pre_approved(n) && !t.approved_always(n), "{n}");
        assert!(t.grant_blocker(n, &json!({})).unwrap().contains("every time"), "{n}");
    }
    assert!(t.pre_approved(PLAN_TOOL) == Base.pre_approved(PLAN_TOOL));
}

#[test]
fn there_is_no_argument_that_could_carry_the_confirmation() {
    let (t, _) = tools();
    for d in t.defs() {
        let props = d.spec.schema["properties"].as_object().unwrap();
        for k in props.keys() {
            assert!(["partition", "filesystem", "label", "new_size"].contains(&k.as_str()), "{} has {k}", d.spec.name);
        }
    }
    // extra arguments a model might invent are ignored: the request is built from the known ones only
    let (t, f) = tools();
    t.call(FORMAT_TOOL, &json!({"partition": "sdc1", "filesystem": "exfat", "label": "BK", "confirm": "sdc1", "typed": "sdc1", "yes": true})).unwrap();
    assert_eq!(f.asked.lock().unwrap().clone(), [Req::Format { part: "sdc1".into(), fs: FormatFs::Exfat, label: "BK".into() }]);
}

#[test]
fn odd_names_sizes_and_file_systems_are_refused_before_the_backend_sees_them() {
    let (t, f) = tools();
    for bad in [json!({"partition": "../etc", "filesystem": "ntfs"}), json!({"partition": "sdc1; rm", "filesystem": "ntfs"}), json!({"partition": "", "filesystem": "ntfs"}), json!({"partition": "sdc1", "filesystem": "btrfs"}), json!({"partition": "sdc1", "filesystem": "ntfs", "label": "a\nb"})] {
        assert!(t.refusal(FORMAT_TOOL, &bad).is_some(), "{bad}");
        assert!(t.call(FORMAT_TOOL, &bad).is_err(), "{bad}");
    }
    for bad in [json!({"partition": "sdc1", "new_size": "lots"}), json!({"partition": "sdc1", "new_size": "1 KB"}), json!({"partition": "sdc1"})] {
        assert!(t.call(RESIZE_TOOL, &bad).is_err(), "{bad}");
    }
    // the plan needs exactly one of file system and size
    assert!(t.call(PLAN_TOOL, &json!({"partition": "sdc1"})).is_err());
    assert!(t.call(PLAN_TOOL, &json!({"partition": "sdc1", "filesystem": "ntfs", "new_size": "1 GB"})).is_err());
    assert!(f.asked.lock().unwrap().is_empty());
}

#[test]
fn the_plan_changes_nothing_and_the_card_is_the_plan() {
    let (t, f) = tools();
    let out = t.call(PLAN_TOOL, &json!({"partition": "sdc1", "filesystem": "exfat"})).unwrap();
    assert!(out.contains("Everything on /dev/sdc1 will be erased"));
    assert!(t.call(PLAN_TOOL, &json!({"partition": "sdc1", "new_size": "8 GB"})).is_ok());
    assert!(f.asked.lock().unwrap().is_empty(), "no request was left by a plan");
    let card = t.preview(FORMAT_TOOL, &json!({"partition": "sdc1", "filesystem": "ntfs"})).unwrap();
    assert!(card.contains("will be erased") && card.contains("cannot do that for you"), "{card}");
    let card = t.preview(RESIZE_TOOL, &json!({"partition": "sda1", "new_size": "8 GB"})).unwrap();
    assert!(card.contains("Cannot be done") && card.contains("never changed"), "{card}");
    assert!(t.describe(FORMAT_TOOL, &json!({"partition": "sdc1", "filesystem": "ntfs"})).contains("erases everything"));
    assert!(t.describe(RESIZE_TOOL, &json!({"partition": "sdc1", "new_size": "8 GB"})).contains("Resize sdc1"));
    // a refusal from the engine reaches the model in words and leaves no request
    assert!(t.call(FORMAT_TOOL, &json!({"partition": "sda1", "filesystem": "ntfs"})).unwrap_err().contains("never changed"));
    assert!(f.asked.lock().unwrap().is_empty());
}

#[test]
fn the_request_file_holds_the_request_and_nothing_to_confirm_with() {
    let dir = std::env::temp_dir().join(format!("iyakki-fmt-req-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = Req::Format { part: "sdc1".into(), fs: FormatFs::Ntfs, label: "My Disk".into() };
    request(&dir, "sam", &a);
    assert_eq!(take_request(&dir, "sam"), Some(a));
    assert_eq!(take_request(&dir, "sam"), None, "taken once");
    let b = Req::Resize { part: "sdc2".into(), bytes: 5 << 30 };
    request(&dir, "sam", &b);
    assert_eq!(take_request(&dir, "sam"), Some(b));
    // somebody else's file, a damaged line, an odd name: dropped
    for junk in ["format\t../x\tntfs\tl", "format\tsdc1\tzfs\t", "resize\tsdc1\t12", "resize\tsdc1\tabc", "wipe\tsdc1", ""] {
        std::fs::write(dir.join("disks-format-request-sam"), junk).unwrap();
        assert_eq!(take_request(&dir, "sam"), None, "{junk:?}");
    }
    request(&dir, "sam", &Req::Resize { part: "sdc2".into(), bytes: 5 << 30 });
    assert_eq!(take_request(&dir, "lee"), None, "one file per person");
    let _ = std::fs::remove_dir_all(&dir);
}
