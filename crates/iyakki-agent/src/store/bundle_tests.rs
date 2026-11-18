//! Tests of app bundles (docs/development/CORE-ACCESS-MODEL.md section 6d): `requires` and the manifest version at install and update, the
//! `contents` members with their own approvals, unknown and unsupported members, all-or-nothing install, update and remove, and the card text.
//! Fakes and temporary folders only; the card itself was only checked as text, never seen on a screen.

use std::cell::RefCell;
use std::collections::BTreeMap;

use iyakki_core_api::appcaps::ApprovalBook;
use iyakki_core_api::bundle::{Member, MemberKind};
use serde_json::json;

use super::bundle::*;
use super::install::tests::{admin, fx, meta, Fx};
use super::install::{record_key, Confirm, InstallError, Review};
use super::tile::Bundle;
use crate::pkg;
use crate::skills::Scope;

fn me() -> Scope {
    Scope::User("root".into())
}

fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: A helper called {name}\ntriggers: {name}\n---\nDo the {name} thing.\n")
}

fn flow_json(id: &str) -> String {
    iyakki_flow::model::Flow::new(id, "Daily").to_json()
}

/// A bundle package (kind tile) whose app.json is built from `manifest` and whose extra files are `files`.
fn bundle_pkg(version: &str, manifest: serde_json::Value, files: &[(&str, String)]) -> Vec<u8> {
    let mut m = json!({"id": "kit", "name": "Kit", "version": version, "icon": "📦"});
    for (k, v) in manifest.as_object().unwrap() {
        m[k] = v.clone();
    }
    let mt = m.to_string();
    let mut all: Vec<(&str, &str)> = vec![("index.html", "<p>kit</p>"), ("app.json", &mt)];
    for (p, c) in files {
        all.push((p, c));
    }
    let b = Bundle::from_files(&all, "kit").unwrap();
    pkg::build_generic("tile", "kit", version, "MIT", &b.to_payload(), Some(&meta("acme", &[]).to_json()), None, None).unwrap().into_bytes()
}

/// Tests never touch the real home folder: flows go under the test's own data folder.
fn flows_home(f: &Fx) -> std::path::PathBuf {
    f.dir.join("flows-home")
}

fn fxb(tag: &str) -> Fx {
    let mut f = fx(tag);
    f.store = f.store.clone().with_flows_home(flows_home(&f));
    f
}

fn review(f: &Fx, bytes: Vec<u8>) -> Review {
    f.store.review(&f.svc, &admin(), bytes, None, "test", false, me(), false, &[])
}

fn install(f: &Fx, r: &Review) -> Result<super::install::Record, InstallError> {
    f.store.install(&f.svc, &admin(), r, Confirm::all())
}

fn full_manifest() -> serde_json::Value {
    json!({
        "manifest_version": 1,
        "requires": {"core_interface": ">=0.1 <1", "capabilities_needed": ["notify"], "capabilities_optional": ["hologram.show"]},
        "contents": [
            {"id": "helper", "kind": "skill", "file": "skills/helper.md", "title": "Helper skill", "capabilities": [{"name": "notify", "why": "tell you when it is done"}]},
            {"id": "daily", "kind": "flow", "file": "flows/daily.json", "title": "Daily run"},
            {"id": "dark", "kind": "theme", "file": "themes/dark.json"},
            {"id": "holo", "kind": "hologram", "file": "holo/h.bin", "optional": true}
        ]
    })
}

fn full_files() -> Vec<(&'static str, String)> {
    vec![("skills/helper.md", skill_md("kit-helper")), ("flows/daily.json", flow_json("kit-daily")), ("themes/dark.json", r#"{"name":"dark"}"#.into()), ("holo/h.bin", "not json at all".into())]
}

// ---------------------------------------------------------------------------------------- requires and manifest_version

#[test]
fn an_install_the_running_core_cannot_serve_is_refused_naming_the_versions_and_the_side_to_update() {
    let f = fxb("bundle-requires");
    let r = review(&f, bundle_pkg("1.0.0", json!({"requires": {"core_interface": ">=0.9 <1"}}), &[]));
    assert!(r.needs_newer_iyakki && !r.can_install());
    let why = r.refusal.clone().unwrap();
    assert!(why.contains("Needs a newer IYAKKI") && why.contains(">=0.9 <1") && why.contains("0.1") && why.contains("Update IYAKKI"), "{why}");
    assert!(matches!(install(&f, &r), Err(InstallError::Refused(_))));
    assert!(f.store.records().is_empty());
    // too new for the app: the other side is named
    let r = review(&f, bundle_pkg("1.0.0", json!({"requires": {"core_interface": "<0.1"}}), &[]));
    assert!(!r.needs_newer_iyakki && r.refusal.as_deref().unwrap().contains("newer version of the app"), "{:?}", r.refusal);
    // a capability the core does not have
    let r = review(&f, bundle_pkg("1.0.0", json!({"requires": {"capabilities_needed": ["hologram.show"]}}), &[]));
    assert!(r.needs_newer_iyakki && r.refusal.as_deref().unwrap().contains("hologram.show"), "{:?}", r.refusal);
    // an optional capability the core lacks only shrinks the app
    assert!(review(&f, bundle_pkg("1.0.0", json!({"requires": {"capabilities_optional": ["hologram.show"]}}), &[])).can_install());
    // a manifest of a newer format
    let r = review(&f, bundle_pkg("1.0.0", json!({"manifest_version": 2, "contents": "a different shape"}), &[]));
    assert!(r.needs_newer_iyakki && r.refusal.as_deref().unwrap().contains("manifest is version 2"), "{:?}", r.refusal);
}

#[test]
fn an_update_that_needs_a_newer_core_says_so_and_leaves_the_installed_version_alone() {
    let f = fxb("bundle-update-requires");
    let r1 = review(&f, bundle_pkg("1.0.0", json!({"requires": {"core_interface": ">=0.1 <1"}}), &[]));
    install(&f, &r1).unwrap();
    let r2 = review(&f, bundle_pkg("1.1.0", json!({"requires": {"core_interface": ">=0.4 <1"}}), &[]));
    assert_eq!(r2.installed.as_deref(), Some("1.0.0"));
    assert!(r2.needs_newer_iyakki && r2.refusal.as_deref().unwrap().starts_with("Needs a newer IYAKKI"), "{:?}", r2.refusal);
    assert!(r2.summary().contains("cannot be installed: Needs a newer IYAKKI"), "{}", r2.summary());
    assert!(install(&f, &r2).is_err());
    assert_eq!(f.store.records().values().next().unwrap().version, "1.0.0");
}

#[test]
fn old_manifests_without_the_new_fields_keep_working_and_unknown_fields_survive_a_round_trip() {
    let f = fxb("bundle-backward");
    let r = review(&f, bundle_pkg("1.0.0", json!({"capabilities": {"storage": true}}), &[]));
    assert!(r.can_install() && r.members.is_empty(), "{}", r.summary());
    install(&f, &r).unwrap();
    let m = json!({"id": "kit", "name": "Kit", "colour": "red", "description": "hello", "future": {"a": [1, 2]}, "manifest_version": 1, "requires": {"core_interface": ">=0.1", "later": true}});
    let b = Bundle::from_files(&[("index.html", "<p>x</p>"), ("app.json", &m.to_string())], "kit").unwrap();
    let again = Bundle::from_payload(&b.to_payload()).unwrap();
    let out = again.manifest.to_json();
    for k in ["colour", "description", "future", "manifest_version"] {
        assert_eq!(out[k], m[k], "{k}");
    }
    assert_eq!(out["requires"], m["requires"]);
    assert_eq!(again.manifest, b.manifest);
}

// ---------------------------------------------------------------------------------------- contents: parse and refuse

#[test]
fn bad_contents_are_refused_when_the_app_is_read() {
    let parse = |contents: serde_json::Value, files: &[(&str, &str)]| {
        let mt = json!({"id": "kit", "name": "Kit", "contents": contents}).to_string();
        let mut all: Vec<(&str, &str)> = vec![("index.html", "<p>x</p>"), ("app.json", &mt)];
        all.extend_from_slice(files);
        Bundle::from_files(&all, "kit").err()
    };
    let one = |extra: serde_json::Value| {
        let mut m = json!({"id": "a", "kind": "skill", "file": "a.md"});
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        json!([m])
    };
    assert!(parse(one(json!({})), &[("a.md", "x")]).is_none());
    assert!(parse(one(json!({"file": "../a.md"})), &[("a.md", "x")]).unwrap().contains("file is a path"));
    assert!(parse(one(json!({"file": "/etc/passwd"})), &[]).unwrap().contains("file is a path"));
    assert!(parse(one(json!({})), &[]).unwrap().contains("not in the app"), "a member file must exist");
    assert!(parse(one(json!({"capabilities": [{"name": "teleport", "why": "x"}]})), &[("a.md", "x")]).unwrap().contains("unknown capability"));
    let big = "x".repeat(513 * 1024);
    assert!(parse(one(json!({})), &[("a.md", &big)]).unwrap().contains("larger than 512 KB"));
    let many: Vec<_> = (0..17).map(|i| json!({"id": format!("m{i}"), "kind": "theme", "file": format!("t{i}.json")})).collect();
    let files: Vec<(String, &str)> = (0..17).map(|i| (format!("t{i}.json"), "{}")).collect();
    let files: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), *b)).collect();
    assert!(parse(json!(many), &files).unwrap().contains("at most 16"));
}

// ---------------------------------------------------------------------------------------- the card and the install

#[test]
fn the_card_groups_by_member_marks_unsupported_ones_and_installs_the_rest() {
    let f = fxb("bundle-card");
    let r = review(&f, bundle_pkg("1.0.0", full_manifest(), &full_files()));
    assert!(r.can_install(), "{}", r.summary());
    let by_id: BTreeMap<&str, &MemberCard> = r.members.iter().map(|m| (m.id.as_str(), m)).collect();
    assert_eq!(by_id.len(), 4);
    assert_eq!(by_id["helper"].lines.len(), 1);
    assert!(by_id["helper"].lines[0].words.contains("tell you when it is done") && by_id["helper"].lines[0].is_new);
    assert_eq!(by_id["holo"].state, MemberState::Skip("not supported by this IYAKKI".into()));
    assert!(by_id["daily"].lines.is_empty() && by_id["daily"].state == MemberState::Install);
    assert!(r.asks_core(), "a member's capabilities need the person's answer");
    let s = r.summary();
    assert!(s.contains("member Helper skill [skill]") && s.contains("member holo [hologram] (not supported by this IYAKKI, left out)"), "{s}");
    // no answer, no install
    assert!(matches!(f.store.install(&f.svc, &admin(), &r, Confirm { permissions: false, warnings: true }), Err(InstallError::NeedsConfirmation(_))));
    assert!(f.svc.skills.get(&me(), "kit-helper").is_none());
    let rec = install(&f, &r).unwrap();
    // each member went where it belongs
    assert!(f.svc.skills.get(&me(), "kit-helper").is_some());
    let flows = iyakki_flow::store::FlowStore::new(&flows_home(&f).join("Flows"), std::sync::Arc::new(|| 0));
    let flow = flows.get_draft("kit-daily").expect("the flow arrived as a draft in the person's Flows folder");
    assert!(!flow.production);
    assert!(flows.published("kit-daily").is_none(), "it never runs until the person publishes it");
    assert_eq!(installed_members(&f.store, &rec.key()).iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["helper", "daily", "dark"]);
    // the approval is per member, for the member's own capabilities only
    let ap = f.store.approval(&format!("{}/helper", rec.key())).unwrap();
    assert!(ap.holds("notify", None) && ap.app == "kit/helper" && ap.app_version == "1.0.0");
    assert!(f.store.approval(&format!("{}/daily", rec.key())).is_none() && f.store.approval(&rec.key()).is_none(), "the app itself holds nothing it did not ask for");
}

#[test]
fn a_member_that_is_unsupported_and_not_optional_blocks_the_install_but_an_optional_one_does_not() {
    let f = fxb("bundle-unsupported");
    for kind in ["hologram", "tool", "mcp", "tile"] {
        let m = json!({"contents": [{"id": "x", "kind": kind, "file": "x.json"}]});
        let r = review(&f, bundle_pkg("1.0.0", m, &[("x.json", "{}".into())]));
        let why = r.refusal.clone().unwrap_or_default();
        assert!(!r.can_install() && why.contains("not supported by this IYAKKI") && why.contains("not marked optional"), "{kind}: {why}");
        let m = json!({"contents": [{"id": "x", "kind": kind, "file": "x.json", "optional": true}]});
        let r = review(&f, bundle_pkg("1.0.0", m, &[("x.json", "{}".into())]));
        assert!(r.can_install() && r.members[0].state == MemberState::Skip("not supported by this IYAKKI".into()), "{kind}: {:?}", r.refusal);
    }
    // a member that asks for a newer core is left out when optional and blocks when not
    let m = |opt: bool| json!({"contents": [{"id": "x", "kind": "theme", "file": "x.json", "optional": opt, "requires": {"core_interface": ">=3"}}]});
    let r = review(&f, bundle_pkg("1.0.0", m(true), &[("x.json", "{}".into())]));
    assert!(r.can_install() && r.members[0].state == MemberState::Skip("needs a newer IYAKKI".into()));
    assert!(!review(&f, bundle_pkg("1.0.0", m(false), &[("x.json", "{}".into())])).can_install());
    // a member file that is not good for its kind is refused whether optional or not
    let bad = json!({"contents": [{"id": "s", "kind": "skill", "file": "s.md", "optional": true}]});
    let r = review(&f, bundle_pkg("1.0.0", bad, &[("s.md", "no front matter".into())]));
    assert!(r.refusal.as_deref().unwrap_or("").contains("is not valid"), "{:?}", r.refusal);
    // flows are personal: for everyone they are not supported
    let m = json!({"contents": [{"id": "f", "kind": "flow", "file": "f.json"}]});
    let r = f.store.review(&f.svc, &admin(), bundle_pkg("1.0.0", m, &[("f.json", flow_json("kit-f"))]), None, "test", false, Scope::Shared, false, &[]);
    assert!(!r.can_install() && r.refusal.as_deref().unwrap().contains("not supported"), "{:?}", r.refusal);
}

// ---------------------------------------------------------------------------------------- all or none

#[test]
fn a_failing_member_rolls_back_the_earlier_ones_and_the_app_itself_in_a_real_install() {
    let f = fxb("bundle-rollback-live");
    // the second member (a flow) will collide with a flow the person already has
    let flows = iyakki_flow::store::FlowStore::new(&flows_home(&f).join("Flows"), std::sync::Arc::new(|| 0));
    flows.create(iyakki_flow::model::Flow::new("kit-daily", "mine"), "root").unwrap();
    let r = review(&f, bundle_pkg("1.0.0", full_manifest(), &full_files()));
    assert!(r.can_install());
    let e = install(&f, &r).unwrap_err().to_string();
    assert!(e.contains("daily") && e.contains("already a flow") && e.contains("Nothing was changed"), "{e}");
    assert!(f.svc.skills.get(&me(), "kit-helper").is_none(), "the skill that went in first was taken out again");
    assert!(f.store.records().is_empty(), "no record of the app");
    assert!(!f.store.dir().join("content").join("user").join("root").join("tile").join("kit.iyp").exists(), "no kept app file");
    assert!(f.store.approval("tile/user/root/kit/helper").is_none());
    assert!(installed_members(&f.store, "tile/user/root/kit").is_empty());
    assert_eq!(flows.get_draft("kit-daily").unwrap().name, "mine", "the person's own flow is untouched");
}

#[derive(Default)]
struct Fake {
    log: RefCell<Vec<String>>,
    live: RefCell<BTreeMap<String, String>>,
    fail_install: Option<&'static str>,
    fail_uninstall: Option<&'static str>,
}

impl MemberBackend for Fake {
    fn supports(&self, _k: &MemberKind) -> bool {
        true
    }
    fn check(&self, _m: &Member, _b: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn install(&self, m: &Member, b: &[u8]) -> Result<(), String> {
        self.log.borrow_mut().push(format!("install {}", m.id));
        if self.fail_install == Some(m.id.as_str()) {
            return Err("disk full".into());
        }
        self.live.borrow_mut().insert(m.id.clone(), String::from_utf8_lossy(b).into());
        Ok(())
    }
    fn uninstall(&self, m: &Member, _b: &[u8]) -> Result<(), String> {
        self.log.borrow_mut().push(format!("uninstall {}", m.id));
        if self.fail_uninstall == Some(m.id.as_str()) {
            return Err("busy".into());
        }
        self.live.borrow_mut().remove(&m.id);
        Ok(())
    }
}

fn member(id: &str, kind: &str, caps: bool) -> (Member, Vec<u8>) {
    let mut m = json!({"id": id, "kind": kind, "file": format!("{id}.txt")});
    if caps {
        m["capabilities"] = json!([{"name": "notify", "why": "tell you"}]);
    }
    (serde_json::from_value(m).unwrap(), format!("{id} v1").into_bytes())
}

fn by<'a>(v: &'a str) -> Approver<'a> {
    Approver { who: "root", bundle: "kit", version: v, sha256: "abc", at: 5 }
}

#[test]
fn install_update_and_remove_are_all_or_none_with_a_backend_that_fails_on_the_third_member() {
    let f = fxb("bundle-atomic");
    let key = "tile/user/root/kit";
    let v1: Vec<_> = vec![member("a", "skill", true), member("b", "flow", false), member("c", "theme", true)];
    // first install: the third fails, the first two are undone, nothing is kept
    let bad = Fake { fail_install: Some("c"), ..Default::default() };
    let e = apply(&f.store, key, &bad, &v1, &by("1.0.0")).err().unwrap();
    assert!(e.contains("\"c\"") && e.contains("disk full") && e.contains("Nothing was changed"), "{e}");
    assert_eq!(*bad.log.borrow(), ["install a", "install b", "install c", "uninstall b", "uninstall a"]);
    assert!(bad.live.borrow().is_empty() && installed_members(&f.store, key).is_empty());
    assert!(f.store.approval(&format!("{key}/a")).is_none() && !f.store.dir().join("bundles").join(key).join("a.member").exists());
    // a good install
    let ok = Fake::default();
    apply(&f.store, key, &ok, &v1, &by("1.0.0")).unwrap();
    assert_eq!(ok.live.borrow().len(), 3);
    assert!(f.store.approval(&format!("{key}/a")).is_some() && f.store.approval(&format!("{key}/b")).is_none());
    // an update whose last member fails puts the old versions back, the dropped member too, and keeps the old approvals
    let new_a = (member("a", "skill", true).0, b"a v2".to_vec());
    let new_d = member("d", "theme", true);
    let v2 = vec![new_a, new_d]; // b and c are dropped, d is new
    let upd = Fake { live: RefCell::new(ok.live.borrow().clone()), fail_install: Some("d"), ..Default::default() };
    let e = apply(&f.store, key, &upd, &v2, &by("2.0.0")).err().unwrap();
    assert!(e.contains("\"d\""), "{e}");
    let live = upd.live.borrow().clone();
    assert_eq!(live.keys().map(String::as_str).collect::<Vec<_>>(), ["a", "b", "c"], "{live:?}");
    assert_eq!(live["a"], "a v1", "the old bytes of a are back");
    assert_eq!(installed_members(&f.store, key).iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    assert_eq!(f.store.approval(&format!("{key}/a")).unwrap().app_version, "1.0.0");
    assert_eq!(std::fs::read(f.store.dir().join("bundles").join(key).join("a.member")).unwrap(), b"a v1");
    // the same update with a working backend: a replaced, d added, b and c gone with their approvals
    let upd = Fake { live: RefCell::new(ok.live.borrow().clone()), ..Default::default() };
    apply(&f.store, key, &upd, &v2, &by("2.0.0")).unwrap();
    assert_eq!(upd.live.borrow().keys().map(String::as_str).collect::<Vec<_>>(), ["a", "d"]);
    assert_eq!(f.store.approval(&format!("{key}/a")).unwrap().app_version, "2.0.0");
    assert!(f.store.approval(&format!("{key}/c")).is_none() && f.store.approval(&format!("{key}/d")).is_some());
    // remove: one that cannot be removed puts the others back
    let stuck = Fake { live: RefCell::new(upd.live.borrow().clone()), fail_uninstall: Some("a"), ..Default::default() };
    let e = remove_all(&f.store, key, &stuck).unwrap_err();
    assert!(e.contains("\"a\"") && e.contains("Nothing was removed"), "{e}");
    assert_eq!(stuck.live.borrow().len(), 2, "d was put back");
    assert_eq!(installed_members(&f.store, key).len(), 2);
    remove_all(&f.store, key, &upd).unwrap();
    assert!(upd.live.borrow().is_empty() && installed_members(&f.store, key).is_empty() && !f.store.dir().join("bundles").join(key).exists());
}

#[test]
fn updating_a_bundle_asks_only_about_new_member_capabilities_and_removing_it_removes_every_member_and_approval() {
    let f = fxb("bundle-update-remove");
    let r1 = review(&f, bundle_pkg("1.0.0", full_manifest(), &full_files()));
    let rec = install(&f, &r1).unwrap();
    // 1.1.0: the skill keeps its capability, the theme is dropped, a second skill asks for something new
    let mut m = full_manifest();
    m["contents"] = json!([
        {"id": "helper", "kind": "skill", "file": "skills/helper.md", "title": "Helper skill", "capabilities": [{"name": "notify", "why": "tell you when it is done"}]},
        {"id": "extra", "kind": "skill", "file": "skills/extra.md", "capabilities": [{"name": "microphone.listen", "why": "hear commands"}]}
    ]);
    let files = vec![("skills/helper.md", skill_md("kit-helper")), ("skills/extra.md", skill_md("kit-extra"))];
    let r2 = review(&f, bundle_pkg("1.1.0", m, &files));
    assert!(r2.can_install(), "{}", r2.summary());
    let helper = r2.members.iter().find(|c| c.id == "helper").unwrap();
    assert!(helper.lines.iter().all(|l| !l.is_new), "already approved");
    let extra = r2.members.iter().find(|c| c.id == "extra").unwrap();
    assert!(extra.lines[0].is_new && extra.lines[0].hard_ask);
    assert!(r2.is_sensitive() && r2.summary().contains("NEW listen through a microphone"), "{}", r2.summary());
    install(&f, &r2).unwrap();
    assert!(f.svc.skills.get(&me(), "kit-extra").is_some() && f.svc.skills.get(&me(), "kit-helper").is_some());
    let flows = iyakki_flow::store::FlowStore::new(&flows_home(&f).join("Flows"), std::sync::Arc::new(|| 0));
    assert!(flows.get_draft("kit-daily").is_none(), "the flow the new version dropped is gone");
    assert!(f.store.approval(&format!("{}/extra", rec.key())).unwrap().holds("microphone.listen", None));
    assert!(f.store.approval(&format!("{}/dark", rec.key())).is_none());
    // remove the app
    f.store.remove(&f.svc, &admin(), "tile", "kit", &me()).unwrap();
    assert!(f.svc.skills.get(&me(), "kit-extra").is_none() && f.svc.skills.get(&me(), "kit-helper").is_none());
    assert!(f.store.records().is_empty() && installed_members(&f.store, &rec.key()).is_empty());
    assert!(f.store.approval(&format!("{}/extra", rec.key())).is_none() && f.store.approval(&format!("{}/helper", rec.key())).is_none());
    let _ = record_key("tile", &me(), "kit");
    let mut book = iyakki_core_api::appcaps::MemoryBook::default();
    book.remove("none");
}

// ---- F-CORE-26: an mcp member is wired for an administrator installing for everyone, and for nobody else

#[test]
fn an_mcp_member_is_installed_removed_and_refused_by_the_rules_of_a_server_that_runs_a_command() {
    use super::bundle::{mcp_member, LiveMembers, MemberBackend};
    use super::install::tests::{admin, user};
    let f = fxb("mcp-member");
    let (m, _) = member("search", "mcp", false);
    let good = br#"{"name": "docs-search", "command": "npx", "args": ["-y", "docs-server"]}"#.to_vec();
    // only an administrator installing for everyone
    let (shared, own) = (Scope::Shared, me());
    let a = admin();
    assert!(LiveMembers::new(&f.store, &f.svc, &a, &shared).supports(&m.kind()));
    assert!(!LiveMembers::new(&f.store, &f.svc, &a, &own).supports(&m.kind()), "for oneself the list of servers is not the person's to change");
    let u = user("alice");
    assert!(!LiveMembers::new(&f.store, &f.svc, &u, &shared).supports(&m.kind()), "a person without administrator rights cannot add a command");
    // the file is checked before anything is written
    let live = LiveMembers::new(&f.store, &f.svc, &a, &shared);
    assert!(live.check(&m, &good).is_ok());
    for bad in [r#"{"name": "x y", "command": "npx"}"#, r#"{"name": "ok", "command": "npx; rm -rf /"}"#, r#"{"name": "ok", "command": "../bin/x"}"#, r#"{"name": "ok", "command": "a b"}"#, r#"{"name": "ok"}"#, r#"{"name": "ok", "command": "npx", "args": [1]}"#, r#"{"name": "ok", "command": "$(id)"}"#, "[]", "no"] {
        assert!(live.check(&m, bad.as_bytes()).is_err(), "{bad}");
    }
    assert!(mcp_member(r#"{"name": "ok", "command": "/usr/bin/tool"}"#).is_ok());
    // install adds it, a second install of the same name is refused, uninstall removes exactly what was added
    live.install(&m, &good).unwrap();
    assert_eq!(f.svc.mcp.list().iter().map(|s| (s.name.as_str(), s.command.as_str(), s.args.len())).collect::<Vec<_>>(), [("docs-search", "npx", 2)]);
    assert!(live.install(&m, &good).unwrap_err().contains("already there"));
    live.uninstall(&m, &good).unwrap();
    assert!(f.svc.mcp.list().is_empty());
    // a server the administrator changed by hand since is left alone when the bundle goes
    live.install(&m, &good).unwrap();
    f.svc.mcp.replace(crate::mcp::StoredServer { name: "docs-search".into(), command: "other-tool".into(), args: vec![], sample: false }).unwrap();
    live.uninstall(&m, &good).unwrap();
    assert_eq!(f.svc.mcp.list()[0].command, "other-tool");
}

// ---- F-CORE-26: tool and nested tile members are kept for an administrator; the card says what an mcp member runs

#[test]
fn tool_and_nested_tile_members_are_kept_only_for_an_administrator_for_everyone_after_their_own_checks() {
    use super::bundle::{nested_tile, tool_member, LiveMembers, MemberBackend};
    use super::install::tests::{admin, user};
    use crate::pkg;
    let f = fxb("tool-tile-members");
    let (tool, _) = member("lookup", "tool", false);
    let (tile, _) = member("child", "tile", false);
    let (shared, own) = (Scope::Shared, me());
    let a = admin();
    for m in [&tool, &tile] {
        assert!(LiveMembers::new(&f.store, &f.svc, &a, &shared).supports(&m.kind()), "{}", m.kind);
        assert!(!LiveMembers::new(&f.store, &f.svc, &a, &own).supports(&m.kind()), "{}: not for oneself", m.kind);
        assert!(!LiveMembers::new(&f.store, &f.svc, &user("alice"), &shared).supports(&m.kind()), "{}: not for a person without administrator rights", m.kind);
    }
    // a tool member describes, it never carries a program
    let live = LiveMembers::new(&f.store, &f.svc, &a, &shared);
    assert_eq!(tool_member(r#"{"name": "lookup", "description": "finds words"}"#).unwrap(), ("lookup".to_string(), "finds words".to_string()));
    for bad in [r#"{"name": "lookup", "command": "sh"}"#, r#"{"name": "lookup", "args": []}"#, r#"{"name": "lookup", "url": "http://x"}"#, r#"{"name": "bad name"}"#, r#"{"description": "x"}"#, "[]", "no"] {
        assert!(live.check(&tool, bad.as_bytes()).is_err(), "{bad}");
    }
    assert!(live.check(&tool, br#"{"name": "lookup"}"#).is_ok());
    // a nested tile is a signed package of its own, of kind tile
    let key = pkg::new_signing_key().unwrap();
    let signed = pkg::build_generic("tile", "child", "1.0.0", "MIT", "{}", None, None, Some(&key)).unwrap();
    let unsigned = pkg::build_generic("tile", "child", "1.0.0", "MIT", "{}", None, None, None).unwrap();
    let skill = pkg::build_generic("skill", "child", "1.0.0", "MIT", "x", None, None, Some(&key)).unwrap();
    assert!(nested_tile(&signed).is_ok() && live.check(&tile, signed.as_bytes()).is_ok());
    assert!(nested_tile(&unsigned).unwrap_err().contains("signed package"), "{:?}", nested_tile(&unsigned));
    assert!(nested_tile(&skill).unwrap_err().contains("not a tile"));
    assert!(nested_tile("not a package").unwrap_err().contains("not a package"));
    // the card tells what they are: kept, not run, not installed
    let note = |m: &Member, b: &[u8]| live.describe(m, b);
    assert!(note(&tool, br#"{"name": "lookup", "description": "finds words"}"#).contains("nothing runs") && note(&tool, br#"{"name": "lookup"}"#).contains("tool lookup"));
    assert!(note(&tile, signed.as_bytes()).contains("not installed or opened"), "{}", note(&tile, signed.as_bytes()));
    // installing them keeps nothing running: the install and uninstall of these kinds change no list of servers or apps
    live.install(&tool, br#"{"name": "lookup"}"#).unwrap();
    live.install(&tile, signed.as_bytes()).unwrap();
    assert!(f.svc.mcp.list().is_empty());
    live.uninstall(&tool, br#"{"name": "lookup"}"#).unwrap();
}

#[test]
fn the_install_card_of_an_mcp_member_names_the_program_it_will_run() {
    use super::bundle::{LiveMembers, MemberBackend};
    use super::install::tests::admin;
    let f = fxb("mcp-card-note");
    let (m, _) = member("search", "mcp", false);
    let a = admin();
    let live = LiveMembers::new(&f.store, &f.svc, &a, &Scope::Shared);
    let n = live.describe(&m, br#"{"name": "docs-search", "command": "npx", "args": ["-y", "docs-server"]}"#);
    assert_eq!(n, "runs this program on the computer when the assistant uses it: npx -y docs-server");
    // a file that is not good gives no note (the plan refuses it with the reason instead)
    assert_eq!(live.describe(&m, br#"{"name": "x y", "command": "npx"}"#), "");
    let (skill, _) = member("s", "skill", false);
    assert_eq!(live.describe(&skill, b"x"), "");
}

#[test]
fn a_tile_reads_the_kept_data_members_of_its_kind_and_nothing_that_is_not_a_json_object() {
    let f = fxb("bundle-readers");
    let key = "tile/user/root/kit";
    let mk = |id: &str, kind: &str, body: &str| (serde_json::from_value::<Member>(json!({"id": id, "kind": kind, "file": format!("{id}.json"), "title": format!("T {id}")})).unwrap(), body.as_bytes().to_vec());
    let v = vec![mk("dark", "theme", r##"{"bg":"#000"}"##), mk("light", "theme", r##"{"bg":"#fff"}"##), mk("weekly", "template", r#"{"title":"w"}"#), mk("broken", "theme", "not json"), mk("helper", "agent", r#"{"prompt":"p"}"#), mk("s", "skill", "x")];
    apply(&f.store, key, &Fake::default(), &v, &by("1.0.0")).unwrap();
    let themes = kept_members(&f.store, "theme");
    assert_eq!(themes.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["dark", "light"], "the broken file is left out: {themes:?}");
    assert_eq!((themes[0].bundle.as_str(), themes[0].title.as_str(), themes[0].value["bg"].as_str()), (key, "T dark", Some("#000")));
    assert_eq!(kept_members(&f.store, "template").len(), 1);
    assert_eq!(kept_members(&f.store, "agent")[0].value["prompt"], "p");
    assert!(kept_members(&f.store, "skill").is_empty() && kept_members(&f.store, "tool").is_empty(), "only the data kinds are read this way");
    // removing the bundle removes what is read
    remove_all(&f.store, key, &Fake::default()).unwrap();
    assert!(kept_members(&f.store, "agent").is_empty());
}


// ---------------------------------------------------------------------------------------- the app's own data migration (F-CORE-28)

#[test]
fn a_wizard_migration_step_runs_the_apps_own_migration_file_over_its_data_with_a_snapshot_and_rolls_back_on_failure() {
    use crate::wizardrun::Env;
    let f = fxb("bundle-migrate");
    let good = r#"{"ops":[{"op":"json_rename_key","file":"settings.json","from":"colour","to":"color"},{"op":"rename","from":"old.txt","to":"new.txt"}]}"#.to_string();
    let bad = r#"{"ops":[{"op":"json_remove_key","file":"settings.json","key":"colour"},{"op":"rename","from":"nothing-here","to":"x"}]}"#.to_string();
    let r = review(&f, bundle_pkg("1.0.0", json!({}), &[("migrations/m1.json", good), ("migrations/m2.json", bad)]));
    let rec = install(&f, &r).unwrap();
    let data = f.store.dir().join("appdata").join(&rec.kind).join(rec.scope.replace('/', "_")).join(&rec.name);
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("settings.json"), r#"{"colour":"red"}"#).unwrap();
    std::fs::write(data.join("old.txt"), "keep me").unwrap();
    let env = crate::wizardtools::RealEnv { svc: f.svc.clone(), user: "root".into(), app: rec.name.clone() };
    // a name that is not a name, and one the package does not have: refused, data untouched
    assert!(env.migrate("../x", None, None, &Default::default()).is_err());
    let e = env.migrate("ghost", None, None, &Default::default()).unwrap_err();
    assert!(e.contains("no migration called ghost") && e.contains("not touched"), "{e}");
    // the second migration fails half way: the snapshot is put back
    let e = env.migrate("m2", None, None, &Default::default()).unwrap_err();
    assert!(e.contains("not there") && e.contains("put back"), "{e}");
    assert_eq!(std::fs::read_to_string(data.join("settings.json")).unwrap(), r#"{"colour":"red"}"#, "the key removed by the first step is back");
    // the first one runs
    assert_eq!(env.migrate("m1", None, None, &Default::default()).unwrap(), "2 migration steps done");
    assert_eq!(std::fs::read_to_string(data.join("new.txt")).unwrap(), "keep me");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(data.join("settings.json")).unwrap()).unwrap();
    assert_eq!(v, json!({"color": "red"}));
    assert!(!f.store.data_snapshots(&record_key_of(&rec)).is_empty(), "a snapshot of the data was taken first");
}

fn record_key_of(rec: &super::install::Record) -> String {
    format!("{}/{}/{}", rec.kind, rec.scope, rec.name)
}

#[test]
fn a_member_acts_through_its_own_broker_session_holding_only_what_it_was_approved_for_and_a_revoke_applies_at_once() {
    use iyakki_core_api::broker::Denial;
    let f = fxb("bundle-session");
    let r = review(&f, bundle_pkg("1.0.0", full_manifest(), &full_files()));
    let rec = install(&f, &r).unwrap();
    let (broker, log) = crate::accessbroker::local(&f.dir, None);
    let key = rec.key();
    let helper = MemberSession::new(broker.clone(), &key, "kit", "helper", "root", "this computer");
    let daily = MemberSession::new(broker.clone(), &key, "kit", "daily", "root", "this computer");
    assert_eq!(helper.decide("notify", None, "tell the person"), Ok(()), "the member asked for notify and the person said yes");
    assert_eq!(helper.decide("camera.view", Some("cam1"), "front door"), Err(Denial::NotGranted), "it never asked for the camera");
    assert_eq!(daily.decide("notify", None, "x"), Err(Denial::NotGranted), "another member of the same bundle holds nothing of its sibling's");
    assert_eq!(MemberSession::new(broker.clone(), &key, "kit", "ghost", "root", "this computer").decide("notify", None, "x"), Err(Denial::NotGranted));
    // the person takes it away in Settings: the next call is refused
    let mut book = crate::store::approvals::FileBook(&f.store);
    let mut ap = book.get(&member_key(&key, "helper")).unwrap();
    assert!(ap.revoke("notify"));
    book.put(&member_key(&key, "helper"), ap);
    assert_eq!(helper.decide("notify", None, "tell the person"), Err(Denial::Revoked));
    // the log names the member as bundle/member and holds no content
    let rows = log.recent(50);
    assert!(rows.iter().any(|e| e.app == "kit/helper" && e.capability == "notify" && e.allowed), "{rows:?}");
    assert!(rows.iter().any(|e| e.app == "kit/helper" && e.capability == "notify" && !e.allowed && e.reason == "revoked"), "{rows:?}");
}

#[test]
fn an_update_whose_package_has_an_on_update_migration_moves_the_apps_data_by_itself_and_a_failing_one_puts_it_back() {
    let f = fxb("bundle-auto-migrate");
    let r1 = review(&f, bundle_pkg("1.0.0", json!({}), &[]));
    let rec = install(&f, &r1).unwrap();
    let data = f.store.dir().join("appdata").join(&rec.kind).join(rec.scope.replace('/', "_")).join(&rec.name);
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("settings.json"), r#"{"colour":"red"}"#).unwrap();
    // an update without a migration file changes nothing
    let r15 = review(&f, bundle_pkg("1.5.0", json!({}), &[]));
    install(&f, &r15).unwrap();
    assert_eq!(f.store.take_last_auto_migration(&rec.key()), "");
    assert_eq!(std::fs::read_to_string(data.join("settings.json")).unwrap(), r#"{"colour":"red"}"#);
    // the next version brings one: it runs after the install, from a snapshot
    let mig = r#"{"ops":[{"op":"json_rename_key","file":"settings.json","from":"colour","to":"color"}]}"#.to_string();
    let r2 = review(&f, bundle_pkg("2.0.0", json!({}), &[("migrations/on-update.json", mig)]));
    install(&f, &r2).unwrap();
    let said = f.store.take_last_auto_migration(&rec.key());
    assert!(said.contains("moved to the layout of 2.0.0"), "{said:?}");
    assert_eq!(std::fs::read_to_string(data.join("settings.json")).unwrap().replace(char::is_whitespace, ""), r#"{"color":"red"}"#);
    assert!(!f.store.data_snapshots(&format!("{}/{}/{}", rec.kind, rec.scope, rec.name)).is_empty(), "the data was copied first");
    // a migration that fails: the data is back as it was and the answer says so
    let bad = r#"{"ops":[{"op":"json_remove_key","file":"settings.json","key":"color"},{"op":"rename","from":"nothing","to":"x"}]}"#.to_string();
    let r3 = review(&f, bundle_pkg("3.0.0", json!({}), &[("migrations/on-update.json", bad)]));
    install(&f, &r3).unwrap();
    let said = f.store.take_last_auto_migration(&rec.key());
    assert!(said.contains("failed") && said.contains("put back"), "{said:?}");
    assert!(std::fs::read_to_string(data.join("settings.json")).unwrap().contains("color"));
    // a fresh install never runs one
    let f2 = fxb("bundle-auto-fresh");
    let r = review(&f2, bundle_pkg("1.0.0", json!({}), &[("migrations/on-update.json", r#"{"ops":[{"op":"delete","file":"x"}]}"#.to_string())]));
    let rec2 = install(&f2, &r).unwrap();
    assert_eq!(f2.store.take_last_auto_migration(&rec2.key()), "");
}
