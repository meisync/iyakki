use serde_json::json;

use super::*;

fn obj(v: Value) -> serde_json::Map<String, Value> {
    v.as_object().unwrap().clone()
}

const KNOWN: [&str; 6] = ["id", "name", "version", "icon", "group", "capabilities"];

fn ext(v: Value) -> ManifestExt {
    ManifestExt::from_object(&obj(v), &KNOWN).unwrap()
}

#[test]
fn version_ranges_read_and_compare() {
    let r = VersionRange::parse(">=0.3 <1").unwrap();
    assert!(r.satisfied_by((0, 3, 0)) && r.satisfied_by((0, 9, 9)));
    assert!(!r.satisfied_by((0, 2, 9)) && !r.satisfied_by((1, 0, 0)));
    assert!(VersionRange::parse("1.2").unwrap().satisfied_by((1, 2, 0)));
    for bad in ["", ">=", ">=x", "~1.2", ">=1.2.3.4", ">=1 <2 <3 <4 <5"] {
        assert!(VersionRange::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn an_old_manifest_without_the_new_fields_means_what_it_meant_and_writes_back_unchanged() {
    let old = json!({"id": "x", "name": "X", "version": "1.0.0", "capabilities": {"storage": true}});
    let e = ext(old);
    assert_eq!(e, ManifestExt::default());
    assert_eq!(e.effective_version(), 1);
    e.validate(&[]).unwrap();
    e.check_core(CORE_INTERFACE).unwrap();
    let mut out = serde_json::Map::new();
    e.write_into(&mut out);
    assert!(out.is_empty(), "nothing is invented for an old manifest");
}

#[test]
fn unknown_fields_and_unknown_requires_fields_are_kept_and_written_back() {
    let m = json!({"id": "x", "manifest_version": 1, "colour": "red", "requires": {"core_interface": ">=0.1", "future_flag": [1, 2]}, "contents": [{"id": "a", "kind": "theme", "file": "a.json", "later": {"k": 1}}]});
    let e = ext(m.clone());
    assert_eq!(e.extra["colour"], json!("red"));
    e.validate(&[]).unwrap();
    let mut out = serde_json::Map::new();
    e.write_into(&mut out);
    for k in ["colour", "manifest_version", "requires", "contents"] {
        assert_eq!(out[k], m[k], "{k}");
    }
}

#[test]
fn requires_names_the_versions_and_the_side_to_update() {
    let r = |s: &str| Requires { core_interface: Some(s.into()), ..Default::default() };
    check_requires(&r(">=0.1 <1"), "0.1").unwrap();
    let e = check_requires(&r(">=0.3 <1"), "0.1").unwrap_err();
    assert_eq!(e.update, UpdateSide::Core);
    assert!(e.message.contains(">=0.3 <1") && e.message.contains("0.1") && e.message.contains("Needs a newer IYAKKI") && e.message.contains("Update IYAKKI"), "{e}");
    let e = check_requires(&r(">=0.1 <0.1"), "0.1").unwrap_err();
    assert_eq!(e.update, UpdateSide::App);
    assert!(e.message.contains("newer version of the app"), "{e}");
    let n = Requires { capabilities_needed: vec!["camera.view".into(), "hologram.show".into()], ..Default::default() };
    let e = check_requires(&n, "0.1").unwrap_err();
    assert_eq!(e.update, UpdateSide::Core);
    assert!(e.message.contains("hologram.show") && !e.message.contains("camera.view,"), "{e}");
    let o = Requires { capabilities_optional: vec!["camera.view".into(), "hologram.show".into()], ..Default::default() };
    check_requires(&o, "0.1").unwrap();
    assert_eq!(missing_optional(&o), ["hologram.show"]);
}

#[test]
fn a_newer_manifest_version_is_kept_unread_and_refused() {
    let m = json!({"id": "x", "manifest_version": 2, "contents": "a new shape", "wizards": 7, "requires": {"anything": true}});
    let e = ext(m.clone());
    assert!(e.is_newer() && e.contents.is_empty() && e.wizards.is_empty() && e.requires.is_none());
    e.validate(&[]).unwrap();
    let err = e.check_core(CORE_INTERFACE).unwrap_err();
    assert_eq!(err.update, UpdateSide::Core);
    assert!(err.message.contains("version 2") && err.message.contains("Needs a newer IYAKKI"), "{err}");
    let mut out = serde_json::Map::new();
    e.write_into(&mut out);
    assert_eq!(Value::Object(out), json!({"manifest_version": 2, "contents": "a new shape", "wizards": 7, "requires": {"anything": true}}));
    assert!(ManifestExt::from_object(&obj(json!({"manifest_version": 0})), &[]).is_err());
    assert!(ManifestExt::from_object(&obj(json!({"manifest_version": "one"})), &[]).is_err());
}

#[test]
fn bad_requires_are_refused() {
    for bad in [json!({"core_interface": "soon"}), json!({"capabilities_needed": ["a b"]}), json!({"capabilities_needed": ["notify", "notify"]})] {
        assert!(validate_requires(&serde_json::from_value(bad.clone()).unwrap()).is_err(), "{bad}");
    }
    let many: Vec<String> = (0..25).map(|i| format!("cap{i}")).collect();
    assert!(validate_requires(&Requires { capabilities_optional: many, ..Default::default() }).unwrap_err().contains("at most 24"));
}

fn members(v: Value) -> Vec<Member> {
    serde_json::from_value(v).unwrap()
}

#[test]
fn contents_accept_known_and_unknown_kinds_and_keep_what_they_do_not_know() {
    let m = members(json!([
        {"id": "helper", "kind": "skill", "file": "skills/helper.md", "capabilities": [{"name": "notify", "why": "tell you"}]},
        {"id": "daily", "kind": "flow", "file": "flows/daily.json", "requires": {"core_interface": ">=0.1"}},
        {"id": "holo", "kind": "hologram", "file": "holo.bin", "optional": true, "tint": "blue"}
    ]));
    validate_contents(&m).unwrap();
    assert_eq!(m[0].kind(), MemberKind::Skill);
    assert_eq!(m[2].kind(), MemberKind::Unknown("hologram".into()));
    assert!(m[2].optional && !m[2].kind().is_known());
    assert_eq!(serde_json::to_value(&m[2]).unwrap()["tint"], "blue");
    assert_eq!(MemberKind::of("theme").name(), "theme");
}

#[test]
fn contents_refuse_bad_paths_ids_duplicates_too_many_and_unknown_capabilities() {
    let one = |extra: Value| {
        let mut m = json!({"id": "a", "kind": "skill", "file": "a.md"});
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        validate_contents(&members(json!([m]))).unwrap_err()
    };
    for bad in ["../a.md", "a/../b.md", "/etc/x", ".hidden", "a//b", "a b.md", ""] {
        assert!(one(json!({"file": bad})).contains("file is a path"), "{bad}");
    }
    assert!(one(json!({"file": "x".repeat(101)})).contains("file is a path"));
    assert!(one(json!({"id": "Bad Id"})).contains("id is"));
    assert!(one(json!({"id": "a/b"})).contains("id is"));
    assert!(one(json!({"kind": "no spaces"})).contains("kind"));
    assert!(one(json!({"capabilities": [{"name": "teleport", "why": "x"}]})).contains("unknown capability"));
    assert!(one(json!({"capabilities": [{"name": "notify", "why": ""}]})).contains("why"));
    assert!(one(json!({"requires": {"core_interface": "soon"}})).contains("core_interface"));
    let dup = members(json!([{"id": "a", "kind": "skill", "file": "a.md"}, {"id": "a", "kind": "skill", "file": "b.md"}]));
    assert!(validate_contents(&dup).unwrap_err().contains("used twice"));
    let dupf = members(json!([{"id": "a", "kind": "skill", "file": "a.md"}, {"id": "b", "kind": "skill", "file": "a.md"}]));
    assert!(validate_contents(&dupf).unwrap_err().contains("used by another"));
    let many: Vec<Value> = (0..17).map(|i| json!({"id": format!("m{i}"), "kind": "theme", "file": format!("t{i}.json")})).collect();
    assert!(validate_contents(&members(json!(many))).unwrap_err().contains("at most 16"));
    assert!(serde_json::from_value::<Vec<Member>>(json!([{"id": "a", "kind": "skill"}])).is_err(), "a member without a file is not a member");
}

#[test]
fn member_files_must_exist_and_stay_under_the_cap() {
    let m = members(json!([{"id": "a", "kind": "skill", "file": "a.md"}, {"id": "b", "kind": "theme", "file": "b.json"}]));
    let sizes = |p: &str| match p {
        "a.md" => Some(10),
        "b.json" => Some(600 * 1024),
        _ => None,
    };
    assert!(validate_member_files(&m, &sizes, 512 * 1024).unwrap_err().contains("larger than 512 KB"));
    assert!(validate_member_files(&m[..1], &sizes, 512 * 1024).is_ok());
    assert!(validate_member_files(&members(json!([{"id": "z", "kind": "skill", "file": "z.md"}])), &sizes, 512 * 1024).unwrap_err().contains("not in the app"));
}

#[test]
fn wizards_in_the_manifest_are_parsed_validated_and_a_member_wizard_uses_the_members_capabilities() {
    let m = json!({"id": "x", "wizards": [{"wizard_version": 1, "id": "w", "trigger": {"on": "on_install"}, "steps": [{"kind": "check", "id": "c", "check": "net.reach", "param": "example.com"}]}]});
    let e = ext(m.clone());
    let net = [CapabilityRequest { name: "net.connect".into(), why: "reach the service".into(), param: None, extra: Default::default() }];
    e.validate(&net).unwrap();
    assert!(e.validate(&[]).unwrap_err().contains("does not declare"));
    let mm = members(json!([{"id": "a", "kind": "tool", "file": "a.json", "wizards": m["wizards"]}]));
    assert!(validate_contents(&mm).unwrap_err().contains("does not declare"), "the app's capabilities do not lend themselves to a member");
    let mut out = serde_json::Map::new();
    e.write_into(&mut out);
    assert_eq!(out["wizards"], m["wizards"]);
}
