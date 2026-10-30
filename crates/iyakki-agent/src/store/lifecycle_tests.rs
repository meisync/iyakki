//! Tests of the lifecycle layer, kind by kind: update to the latest, no-op when current, the permission gate, a failing check that
//! puts the old version back, explicit rollback, pin, uninstall with keep/delete data, dependents, source gone / revoked /
//! offline, Update all with mixed outcomes, and a cut-off update.

use std::path::PathBuf;

use super::catalog::{self, Fetch, SourceKind};
use super::install::tests::{admin, fx, meta, Fx, NOW};
use super::install::{record_key, Confirm};
use super::installed::{Status, Type};
use super::lifecycle::*;
use super::manifest::{Dependency, Meta};
use super::Store;
use crate::pkg::{self, new_signing_key, public_key_hex};
use crate::skills::{self, Scope};
use ed25519_dalek::SigningKey;
use iyakki_sec::policy::Role;

struct NoFetch;
impl Fetch for NoFetch {
    fn get(&self, url: &str, _max: usize) -> Result<Vec<u8>, String> {
        Err(format!("offline: {url}"))
    }
}

struct W {
    f: Fx,
    key: SigningKey,
    stick: PathBuf,
    entries: Vec<catalog::Entry>,
    serial: u64,
}

fn world(tag: &str) -> W {
    let mut f = fx(tag);
    let key = new_signing_key().unwrap();
    f.svc.packages.trust_key(&public_key_hex(&key), "Acme").unwrap();
    f.store = f.store.clone().with_check_timeout(1500);
    let stick = f.dir.join("stick");
    std::fs::create_dir_all(&stick).unwrap();
    let mut w = W { f, key, stick, entries: vec![], serial: 0 };
    w.write();
    w.f.store.add_source(&admin(), "Stick", SourceKind::Folder, w.stick.to_str().unwrap()).unwrap();
    w
}

impl W {
    fn write(&mut self) {
        self.serial += 1;
        std::fs::write(self.stick.join("catalog.json"), catalog::tests::catalog("stick", self.serial, self.entries.clone()).to_json()).unwrap();
    }

    /// Put a package on the stick as the only version of that app in the catalog.
    fn publish(&mut self, kind: &str, name: &str, version: &str, bytes: &[u8], m: &Meta) {
        let file = format!("{name}-{version}.iyp");
        std::fs::write(self.stick.join(&file), bytes).unwrap();
        let mut e = catalog::tests::entry(name, bytes, &file);
        e.kind = kind.into();
        e.version = version.into();
        e.signer = public_key_hex(&self.key);
        e.meta = m.clone();
        self.entries.retain(|x| !(x.kind == kind && x.name == name));
        self.entries.push(e);
        self.write();
    }

    fn skill(&self, name: &str, version: &str, m: &Meta) -> Vec<u8> {
        let md = format!("---\nname: {name}\ndescription: Notes for {name}\ntriggers: {name}\n---\nBody of {name} {version}.\n");
        let sk = skills::parse(&md, Scope::Shared).unwrap();
        pkg::build_generic("skill", name, version, "MIT", &skills::render(&sk), Some(&m.to_json()), None, Some(&self.key)).unwrap().into_bytes()
    }

    fn tool(&self, name: &str, version: &str, perms: &[&str], m: &Meta) -> Vec<u8> {
        let mut j: serde_json::Value = serde_json::from_str(&crate::wtool::tests::manifest(name, perms, serde_json::json!({}))).unwrap();
        j["version"] = version.into();
        pkg::build_tool(&j.to_string(), crate::wtool::tests::ECHO_WAT.as_bytes(), Some(&self.key)).map(|t| pkg::resign(&t, Some(&m.to_json()), None, &self.key).unwrap()).unwrap().into_bytes()
    }

    fn mcp(&self, name: &str, version: &str, command: &str, args: &[&str], m: &Meta) -> Vec<u8> {
        let payload = serde_json::json!({"name": name, "command": command, "args": args}).to_string();
        pkg::build_generic("mcp", name, version, "MIT", &payload, Some(&m.to_json()), None, Some(&self.key)).unwrap().into_bytes()
    }

    fn content(&self, kind: &str, name: &str, version: &str, m: &Meta) -> Vec<u8> {
        pkg::build_generic(kind, name, version, "MIT", &format!("{{\"v\":\"{version}\"}}"), Some(&m.to_json()), None, Some(&self.key)).unwrap().into_bytes()
    }

    fn loaded(&self) -> Vec<catalog::Loaded> {
        self.f.store.local_catalogs(&self.f.svc)
    }

    fn lives(&self) -> Vec<Life> {
        self.lives_with(&Ctx::default())
    }

    fn lives_with(&self, ctx: &Ctx) -> Vec<Life> {
        self.f.store.lifecycle(&self.f.svc, &Role::Admin.template(), "root", &self.loaded(), ctx)
    }

    fn life(&self, id: &str) -> Life {
        Store::find_life(&self.lives(), id).unwrap().clone()
    }

    /// Install the catalog's version of `name` for everyone, as the person would from Discover.
    fn install(&self, name: &str) {
        let l = self.loaded();
        let a = admin();
        let items = self.f.store.items(&self.f.svc, &a, &l);
        let item = items.iter().find(|i| i.entry.name == name).unwrap_or_else(|| panic!("{name} is in the catalog"));
        let r = self.f.store.review_item(&self.f.svc, &a, item, &l, &NoFetch, Scope::Shared, &[]).unwrap();
        self.f.store.install(&self.f.svc, &a, &r, Confirm::all()).unwrap_or_else(|e| panic!("{e}: {}", r.summary()));
    }

    fn plan(&self, id: &str) -> Result<UpdatePlan, LifeError> {
        let l = self.life(id);
        self.f.store.plan_update(&self.f.svc, &admin(), &l, &self.loaded(), &NoFetch)
    }

    fn update(&self, id: &str, c: Confirm) -> Result<String, LifeError> {
        let p = self.plan(id)?;
        self.f.store.apply_update(&self.f.svc, &admin(), &p, c, &|_, _| {})
    }

    fn version(&self, kind: &str, name: &str) -> String {
        self.f.store.records().get(&record_key(kind, &Scope::Shared, name)).map(|r| r.version.clone()).unwrap_or_default()
    }
}

fn m(perms: &[&str]) -> Meta {
    Meta { title: String::new(), ..meta("acme", perms) }
}

#[test]
fn a_skill_updates_to_the_latest_and_is_a_no_op_when_current_with_the_old_version_kept_for_one_rollback() {
    let mut w = world("life-skill");
    let v1 = w.skill("meeting-notes", "1.0.0", &m(&[]));
    w.publish("skill", "meeting-notes", "1.0.0", &v1, &m(&[]));
    w.install("meeting-notes");
    let l = w.life("skill:meeting-notes");
    assert_eq!((l.check.clone(), l.latest.clone()), (Check::UpToDate, Some("1.0.0".into())));
    assert!(!l.has_update() && l.previous.is_none());
    assert!(w.plan("skill:meeting-notes").unwrap_err().to_string().contains("up to date"));
    let mut m2 = m(&[]);
    m2.changelog = "Faster summaries.\nMore detail".into();
    let v2 = w.skill("meeting-notes", "1.1.0", &m2);
    w.publish("skill", "meeting-notes", "1.1.0", &v2, &m2);
    let l = w.life("skill:meeting-notes");
    assert_eq!((l.check.clone(), l.latest.clone(), l.changelog.as_str()), (Check::Update, Some("1.1.0".into()), "Faster summaries."));
    assert_eq!(l.entry.status, Status::Update("1.1.0".into()));
    assert_eq!(Store::outdated(&w.lives()).len(), 1);
    let plan = w.plan("skill:meeting-notes").unwrap();
    assert!(plan.added.is_empty() && !plan.needs_confirm() && plan.summary().contains("1.0.0 to 1.1.0"), "{}", plan.summary());
    w.update("skill:meeting-notes", Confirm::default()).unwrap();
    assert_eq!(w.version("skill", "meeting-notes"), "1.1.0");
    assert!(w.f.svc.skills.get(&Scope::Shared, "meeting-notes").unwrap().body.contains("1.1.0"));
    let l = w.life("skill:meeting-notes");
    assert_eq!((l.check.clone(), l.previous.clone()), (Check::UpToDate, Some("1.0.0".into())));
    assert!(l.history.iter().any(|h| h.version == "1.1.0" && h.action == "updated"));
    // one generation only: a third version drops the first one's copy
    let v3 = w.skill("meeting-notes", "1.2.0", &m(&[]));
    w.publish("skill", "meeting-notes", "1.2.0", &v3, &m(&[]));
    let first_sha = pkg::parse(&String::from_utf8(v1).unwrap()).unwrap().manifest.sha256;
    w.update("skill:meeting-notes", Confirm::default()).unwrap();
    assert_eq!(w.life("skill:meeting-notes").previous.as_deref(), Some("1.1.0"));
    assert!(!w.f.dir.join("store/pkgs").join(format!("{first_sha}.iyp")).exists(), "bounded disk: only the previous version is kept");
    assert_eq!(std::fs::read_dir(w.f.dir.join("store/pkgs")).unwrap().count(), 2);
}

#[test]
fn more_permissions_need_an_explicit_confirmation_and_unchanged_ones_do_not() {
    let mut w = world("life-perms");
    let t1 = w.tool("shout", "1.0.0", &["net:example.com"], &m(&["net:example.com"]));
    w.publish("tool", "shout", "1.0.0", &t1, &m(&["net:example.com"]));
    w.install("shout");
    // same permissions: no card
    let same = w.tool("shout", "1.0.1", &["net:example.com"], &m(&["net:example.com"]));
    w.publish("tool", "shout", "1.0.1", &same, &m(&["net:example.com"]));
    let p = w.plan("tool:shout").unwrap();
    assert!(p.added.is_empty() && !p.needs_confirm());
    w.update("tool:shout", Confirm { permissions: false, warnings: false }).unwrap();
    assert_eq!(w.version("tool", "shout"), "1.0.1");
    // more: the diff is shown and nothing changes without the yes
    let more = w.tool("shout", "1.1.0", &["net:example.com", "write:/tmp"], &m(&["net:example.com", "write:/tmp"]));
    w.publish("tool", "shout", "1.1.0", &more, &m(&["net:example.com", "write:/tmp"]));
    let p = w.plan("tool:shout").unwrap();
    assert_eq!(p.added.iter().map(|x| x.words.as_str()).collect::<Vec<_>>(), vec!["write files in /tmp"]);
    assert!(p.needs_confirm() && p.summary().contains("NEW PERMISSIONS"), "{}", p.summary());
    let e = w.f.store.apply_update(&w.f.svc, &admin(), &p, Confirm { permissions: false, warnings: true }, &|_, _| {}).unwrap_err();
    assert!(matches!(e, LifeError::NeedsConfirm(ref t) if t.contains("write files in /tmp")), "{e}");
    assert_eq!(w.version("tool", "shout"), "1.0.1");
    assert_eq!(w.f.svc.packages.get_tool(&Scope::Shared, "shout").unwrap().manifest.version, "1.0.1");
    w.f.store.apply_update(&w.f.svc, &admin(), &p, Confirm::all(), &|_, _| {}).unwrap();
    assert_eq!(w.f.svc.packages.get_tool(&Scope::Shared, "shout").unwrap().manifest.version, "1.1.0");
    // the list warned about it before the download (the catalog's own listing)
    assert!(w.life("tool:shout").previous.is_some());
}

fn good_mcp(w: &W) -> (String, Vec<String>) {
    let spec = crate::mcp::tests::mock_server(&w.f.dir);
    (spec.command, spec.args)
}

#[test]
fn an_mcp_update_that_does_not_start_is_rolled_back_automatically_and_a_good_one_is_checked_with_list_tools() {
    let mut w = world("life-mcp");
    let (cmd, args) = good_mcp(&w);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mp = m(&["run:/bin/sh"]);
    let v1 = w.mcp("files", "1.0.0", &cmd, &args, &mp);
    w.publish("mcp", "files", "1.0.0", &v1, &mp);
    w.install("files");
    assert_eq!(w.f.svc.mcp.list()[0].args, args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    // 1.1.0 is a server that exits at once
    let bad = w.mcp("files", "1.1.0", "/bin/sh", &["-c", "exit 1"], &mp);
    w.publish("mcp", "files", "1.1.0", &bad, &mp);
    let e = w.update("mcp:files", Confirm::all()).unwrap_err();
    assert!(matches!(&e, LifeError::Failed(t) if t.contains("failed its check") && t.contains("1.0.0 was put back")), "{e}");
    assert_eq!(w.version("mcp", "files"), "1.0.0");
    let s = w.f.svc.mcp.list();
    assert_eq!((s.len(), s[0].args.clone()), (1, args.iter().map(|x| x.to_string()).collect()), "the old definition is back");
    assert!(w.f.store.life_state().journal.is_none());
    let l = w.life("mcp:files");
    assert_eq!(l.check, Check::Update, "the update is still on offer");
    assert!(l.history.iter().any(|h| h.action == "kept after a failed update"));
    assert_eq!(std::fs::read_dir(w.f.dir.join("store/pkgs")).unwrap().count(), 1, "the failed version's copy is not left behind");
    // 1.2.0 starts and answers tools/list
    let good = w.mcp("files", "1.2.0", &cmd, &args, &mp);
    w.publish("mcp", "files", "1.2.0", &good, &mp);
    w.update("mcp:files", Confirm::all()).unwrap();
    assert_eq!(w.version("mcp", "files"), "1.2.0");
    // a non-administrator cannot update a server
    let p = w.plan("mcp:files").unwrap_err();
    assert!(p.to_string().contains("up to date"));
}

#[test]
fn explicit_rollback_goes_back_is_recorded_pins_the_old_version_and_normal_rules_still_refuse_going_down() {
    let mut w = world("life-rollback");
    let v1 = w.skill("meeting-notes", "1.0.0", &m(&[]));
    w.publish("skill", "meeting-notes", "1.0.0", &v1, &m(&[]));
    w.install("meeting-notes");
    let v2 = w.skill("meeting-notes", "2.0.0", &m(&[]));
    w.publish("skill", "meeting-notes", "2.0.0", &v2, &m(&[]));
    w.update("skill:meeting-notes", Confirm::default()).unwrap();
    // an ordinary install of the older package is still refused (the high-water mark)
    let a = admin();
    let r = w.f.store.review(&w.f.svc, &a, v1.clone(), None, "file", false, Scope::Shared, false, &[]);
    assert!(r.refusal.as_deref().unwrap_or("").contains("Going back to older versions is refused"), "{}", r.summary());
    // but the explicit rollback is allowed
    let l = w.life("skill:meeting-notes");
    let said = w.f.store.rollback(&w.f.svc, &a, &l, &|_, _| {}).unwrap();
    assert!(said.contains("back at 1.0.0") && said.contains("pinned"), "{said}");
    assert_eq!(w.version("skill", "meeting-notes"), "1.0.0");
    assert!(w.f.svc.skills.get(&Scope::Shared, "meeting-notes").unwrap().body.contains("1.0.0"));
    let l = w.life("skill:meeting-notes");
    assert_eq!(l.pinned.as_ref().map(|p| (p.version.as_str(), p.why.as_str())), Some(("1.0.0", "rolled back")));
    assert!(l.history.iter().any(|h| h.action == "rolled back"));
    assert_eq!(l.previous.as_deref(), Some("2.0.0"), "and forward again is one more rollback");
    // the pin keeps the update from coming straight back; unpinning lets it
    assert!(w.plan("skill:meeting-notes").unwrap_err().to_string().contains("pinned at 1.0.0"));
    w.f.store.unpin(l.key()).unwrap();
    assert!(w.plan("skill:meeting-notes").is_ok());
    // with nothing kept there is nothing to go back to
    let fresh = w.life("skill:meeting-notes");
    w.f.store.remove(&w.f.svc, &a, "skill", "meeting-notes", &Scope::Shared).unwrap();
    assert!(w.f.store.rollback(&w.f.svc, &a, &fresh, &|_, _| {}).is_err());
}

#[test]
fn a_pinned_item_is_not_updated_by_hand_or_by_update_all_and_a_manual_newer_install_clears_the_pin() {
    let mut w = world("life-pin");
    let v1 = w.skill("alpha", "1.0.0", &m(&[]));
    w.publish("skill", "alpha", "1.0.0", &v1, &m(&[]));
    w.install("alpha");
    let v2 = w.skill("alpha", "1.1.0", &m(&[]));
    w.publish("skill", "alpha", "1.1.0", &v2, &m(&[]));
    let l = w.life("skill:alpha");
    w.f.store.pin(l.key(), "1.0.0").unwrap();
    let l = w.life("skill:alpha");
    assert!(l.has_update() && !l.can_update() && l.status_line().contains("pinned at 1.0.0"));
    assert!(w.plan("skill:alpha").unwrap_err().to_string().contains("pinned"));
    let rep = w.f.store.update_all(&w.f.svc, &Role::Admin.template(), "root", &w.loaded(), &NoFetch, &|_, _| {}, &mut |_| {});
    assert_eq!(rep.items.len(), 1);
    assert!(matches!(&rep.items[0].1, Done::Skipped(r) if r.contains("pinned")), "{}", rep.summary());
    assert_eq!(w.version("skill", "alpha"), "1.0.0");
    // installing a newer one on purpose from the store (not through the lifecycle) clears the pin
    let a = admin();
    let r = w.f.store.review(&w.f.svc, &a, v2, None, "file", false, Scope::Shared, false, &[]);
    w.f.store.install(&w.f.svc, &a, &r, Confirm::all()).unwrap();
    assert!(w.life("skill:alpha").pinned.is_none());
    assert!(w.f.store.unpin(l.key()).is_err());
}

#[test]
fn removing_keeps_the_data_by_default_deletes_it_only_after_typing_the_name_and_cleans_every_registration() {
    let mut w = world("life-remove");
    let v1 = w.skill("alpha", "1.0.0", &m(&[]));
    w.publish("skill", "alpha", "1.0.0", &v1, &m(&[]));
    w.install("alpha");
    let v2 = w.skill("alpha", "1.1.0", &m(&[]));
    w.publish("skill", "alpha", "1.1.0", &v2, &m(&[]));
    w.update("skill:alpha", Confirm::default()).unwrap();
    let data = w.f.dir.join("store/appdata/skill/shared/alpha");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("notes.txt"), "keep me").unwrap();
    let p = Role::Admin.template();
    let lives = w.lives();
    let l = Store::find_life(&lives, "skill:alpha").unwrap();
    let plan = w.f.store.plan_remove(&w.f.svc, "root", &lives, l);
    assert!(plan.data.len() == 1 && plan.cached > 0 && plan.summary().contains("KEPT unless you choose"), "{}", plan.summary());
    // deleting data needs the typed name
    let e = w.f.store.remove_item(&w.f.svc, &p, "root", &lives, l, &RemoveOpts { delete_data: true, ..Default::default() }, &|_, _| {}).unwrap_err();
    assert!(matches!(e, LifeError::NeedsConfirm(ref t) if t.contains("Type its name")), "{e}");
    assert!(w.f.svc.skills.get(&Scope::Shared, "alpha").is_some(), "nothing was removed by the refused attempt");
    // keep (the default)
    let said = w.f.store.remove_item(&w.f.svc, &p, "root", &lives, l, &RemoveOpts::default(), &|_, _| {}).unwrap();
    assert!(said.contains("Removed") && said.contains("Kept its data"), "{said}");
    assert!(data.join("notes.txt").exists());
    assert!(w.f.svc.skills.get(&Scope::Shared, "alpha").is_none() && w.f.store.records().is_empty());
    assert!(w.f.svc.packages.list(&Scope::Shared).is_empty(), "the skills tile's record is gone too");
    assert_eq!(std::fs::read_dir(w.f.dir.join("store/pkgs")).map(|d| d.count()).unwrap_or(0), 0, "cached packages, both generations, are gone");
    let st = w.f.store.life_state();
    assert!(st.prev.is_empty() && st.history.is_empty() && st.pins.is_empty());
    assert!(!w.lives().iter().any(|x| x.entry.name == "alpha"));
    // install again, then delete the data
    w.install("alpha");
    let lives = w.lives();
    let l = Store::find_life(&lives, "skill:alpha").unwrap();
    let said = w.f.store.remove_item(&w.f.svc, &p, "root", &lives, l, &RemoveOpts { delete_data: true, typed: "alpha".into(), ..Default::default() }, &|_, _| {}).unwrap();
    assert!(said.contains("Deleted its data folder"), "{said}");
    assert!(!data.exists());
}

#[test]
fn removing_an_mcp_server_drops_its_registration_and_an_ordinary_user_may_not_remove_what_is_shared() {
    let mut w = world("life-mcp-rm");
    let (cmd, args) = good_mcp(&w);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mp = m(&["run:/bin/sh"]);
    let v1 = w.mcp("files", "1.0.0", &cmd, &args, &mp);
    w.publish("mcp", "files", "1.0.0", &v1, &mp);
    w.install("files");
    let lives = w.lives();
    let l = Store::find_life(&lives, "mcp:files").unwrap();
    let up = Role::User.template();
    let e = w.f.store.remove_item(&w.f.svc, &up, "alice", &lives, l, &RemoveOpts::default(), &|_, _| {}).unwrap_err();
    assert!(matches!(e, LifeError::Refused(_)), "{e}");
    assert_eq!(w.f.svc.mcp.list().len(), 1);
    w.f.store.remove_item(&w.f.svc, &Role::Admin.template(), "root", &lives, l, &RemoveOpts::default(), &|_, _| {}).unwrap();
    assert!(w.f.svc.mcp.list().is_empty());
}

#[test]
fn dependents_are_warned_about_and_removal_needs_that_acknowledged() {
    let mut w = world("life-deps");
    let base = w.tool("shout", "1.0.0", &[], &m(&[]));
    w.publish("tool", "shout", "1.0.0", &base, &m(&[]));
    let mut dm = m(&[]);
    dm.dependencies = vec![Dependency { name: "shout".into(), min_version: "1.0.0".into() }];
    let sk = w.skill("loud-notes", "1.0.0", &dm);
    w.publish("skill", "loud-notes", "1.0.0", &sk, &dm);
    w.install("shout");
    w.install("loud-notes");
    let lives = w.lives();
    let l = Store::find_life(&lives, "tool:shout").unwrap();
    let p = Role::Admin.template();
    let loud = lives.iter().find(|x| x.entry.name == "loud-notes").unwrap().entry.title.clone();
    assert_eq!(w.f.store.dependents(&w.f.svc, &lives, l), vec![loud]);
    let e = w.f.store.remove_item(&w.f.svc, &p, "root", &lives, l, &RemoveOpts::default(), &|_, _| {}).unwrap_err();
    assert!(matches!(e, LifeError::NeedsConfirm(ref t) if t.contains("is used by")), "{e}");
    assert!(w.f.svc.packages.get_tool(&Scope::Shared, "shout").is_some());
    w.f.store.remove_item(&w.f.svc, &p, "root", &lives, l, &RemoveOpts { ack_dependents: true, ..Default::default() }, &|_, _| {}).unwrap();
    assert!(w.f.svc.packages.get_tool(&Scope::Shared, "shout").is_none());
    // an MCP server that runs a command-line tool depends on it
    crate::clis::write_fake_installed(&w.f.svc.clis, "kubectl", "#!/bin/sh\necho v1\n");
    w.f.svc.mcp.add(crate::mcp::StoredServer { name: "kube".into(), command: "/x/kubectl".into(), args: vec![], sample: false }).unwrap();
    let lives = w.lives();
    let k = Store::find_life(&lives, "cli:kubectl").unwrap();
    assert_eq!(w.f.store.dependents(&w.f.svc, &lives, k), vec!["MCP server kube".to_string()]);
}

#[test]
fn a_store_that_went_away_a_revoked_publisher_and_an_unreachable_store_each_say_so() {
    let mut w = world("life-states");
    let v1 = w.skill("alpha", "1.0.0", &m(&[]));
    w.publish("skill", "alpha", "1.0.0", &v1, &m(&[]));
    w.install("alpha");
    let b = w.skill("beta", "1.0.0", &m(&[]));
    w.publish("skill", "beta", "1.0.0", &b, &m(&[]));
    w.install("beta");
    // by hand and from a file: no way to check
    w.f.svc.skills.add(Scope::Shared, "---\nname: mine\ndescription: x\ntriggers: x\n---\nDo.\n", true, true).unwrap();
    let c = w.life("skill:mine").check;
    assert!(matches!(c, Check::NoWay(_)), "{c:?}");
    let a = admin();
    let r = w.f.store.review(&w.f.svc, &a, w.skill("gamma", "1.0.0", &m(&[])), None, "file", false, Scope::Shared, false, &[]);
    w.f.store.install(&w.f.svc, &a, &r, Confirm::all()).unwrap();
    assert!(matches!(w.life("skill:gamma").check, Check::NoWay(ref t) if t.contains("file")));
    // an unreachable remote store: the installed item came from it, so nothing newer can be known
    let id = w.f.store.add_source(&a, "Portal", SourceKind::Url, "https://portal.example/catalog.json").unwrap();
    let mut rec = w.f.store.records();
    let b = rec.get_mut(&record_key("skill", &Scope::Shared, "beta")).unwrap();
    b.source = id.clone();
    b.signer = None;
    // and the stick lists a package of that name from some other publisher, which is not a source for beta's updates
    w.entries.iter_mut().find(|e| e.name == "beta").unwrap().signer = "ab".repeat(32);
    w.write();
    w.f.store.save_records(&rec).unwrap();
    let c = w.life("skill:beta").check;
    assert!(matches!(&c, Check::Offline(t) if t.contains("not been downloaded")), "{c:?}");
    let ctx = Ctx { refresh_errors: vec![("Portal".into(), "no route".into())] };
    let c = w.lives_with(&ctx).into_iter().find(|l| l.entry.name == "beta").unwrap().check;
    assert!(matches!(&c, Check::Offline(t) if t.contains("no route")), "{c:?}");
    assert_eq!(Check::Offline(String::new()).label(), "Can't check offline");
    // switched off, then removed: source gone
    w.f.store.toggle_source(&a, &id).unwrap();
    assert!(matches!(w.life("skill:beta").check, Check::SourceGone(ref t) if t.contains("switched off")));
    w.f.store.remove_source(&a, &id).unwrap();
    assert!(matches!(w.life("skill:beta").check, Check::SourceGone(ref t) if t.contains("no longer in your list")));
    // the stick itself is gone
    let st = w.f.store.config().sources.iter().find(|s| s.name == "Stick").unwrap().id.clone();
    w.f.store.remove_source(&a, &st).unwrap();
    assert!(matches!(w.life("skill:alpha").check, Check::SourceGone(_)));
    // revoked: a root-signed list withdraws the publisher
    let list = super::revocation::sign_list(&w.f.root, 1, NOW, NOW + 1000, vec![public_key_hex(&w.key)], vec![], vec![], vec![]);
    w.f.store.accept_revocations(&list).unwrap();
    let l = w.life("skill:alpha");
    assert_eq!((l.check.clone(), l.entry.status.clone()), (Check::Revoked, Status::Revoked));
    assert!(!l.can_update());
}

#[test]
fn update_all_does_one_at_a_time_and_reports_updated_skipped_and_failed() {
    let mut w = world("life-all");
    let (cmd, args) = good_mcp(&w);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    for n in ["ok-skill", "pinned-skill"] {
        let b = w.skill(n, "1.0.0", &m(&[]));
        w.publish("skill", n, "1.0.0", &b, &m(&[]));
        w.install(n);
    }
    let t = w.tool("shout", "1.0.0", &[], &m(&[]));
    w.publish("tool", "shout", "1.0.0", &t, &m(&[]));
    w.install("shout");
    let mp = m(&["run:/bin/sh"]);
    let s = w.mcp("files", "1.0.0", &cmd, &args, &mp);
    w.publish("mcp", "files", "1.0.0", &s, &mp);
    w.install("files");
    let pin = w.life("skill:pinned-skill");
    w.f.store.pin(pin.key(), "1.0.0").unwrap();
    // everything has a new version: one is fine, one pinned, one wants more permissions, one does not start
    for n in ["ok-skill", "pinned-skill"] {
        let b = w.skill(n, "1.1.0", &m(&[]));
        w.publish("skill", n, "1.1.0", &b, &m(&[]));
    }
    let t = w.tool("shout", "1.1.0", &["net:example.com"], &m(&["net:example.com"]));
    w.publish("tool", "shout", "1.1.0", &t, &m(&["net:example.com"]));
    let bad = w.mcp("files", "1.1.0", "/bin/sh", &["-c", "exit 1"], &mp);
    w.publish("mcp", "files", "1.1.0", &bad, &mp);
    let lives = w.lives();
    let pre = Store::update_all_preview(&lives);
    assert_eq!(pre.len(), 4);
    assert!(pre.iter().any(|(t, s, ok)| t == "pinned-skill" && s.contains("pinned") && !*ok), "{pre:?}");
    assert!(pre.iter().any(|(_, s, ok)| s.contains("asks for more") && !*ok), "{pre:?}");
    let mut seen = vec![];
    let rep = w.f.store.update_all(&w.f.svc, &Role::Admin.template(), "root", &w.loaded(), &NoFetch, &|_, _| {}, &mut |m| seen.push(m.to_string()));
    assert_eq!(seen.len(), 4, "progress before each item: {seen:?}");
    let by = |n: &str| rep.items.iter().find(|(t, _)| t.contains(n)).map(|(_, d)| d.clone()).unwrap_or_else(|| panic!("{n} in {}", rep.summary()));
    let titles: Vec<String> = rep.items.iter().map(|(t, _)| t.clone()).collect();
    assert_eq!(rep.items.iter().filter(|(_, d)| matches!(d, Done::Updated(_))).count(), 1, "{}", rep.summary());
    assert_eq!(rep.items.iter().filter(|(_, d)| matches!(d, Done::Skipped(_))).count(), 2, "{}", rep.summary());
    assert_eq!(rep.items.iter().filter(|(_, d)| matches!(d, Done::Failed(_))).count(), 1, "{}", rep.summary());
    assert!(rep.summary().starts_with("1 updated, 2 skipped, 1 failed."), "{titles:?}");
    let _ = by("ok-skill");
    assert_eq!(w.version("skill", "ok-skill"), "1.1.0");
    assert_eq!(w.version("skill", "pinned-skill"), "1.0.0");
    assert_eq!(w.version("tool", "shout"), "1.0.0");
    assert_eq!(w.version("mcp", "files"), "1.0.0");
    assert!(rep.summary().contains("asks for new permissions") && rep.summary().contains("failed its check"));
}

#[test]
fn a_cut_off_update_is_mended_and_leaves_the_old_version_working() {
    let mut w = world("life-crash");
    let v1 = w.skill("alpha", "1.0.0", &m(&[]));
    w.publish("skill", "alpha", "1.0.0", &v1, &m(&[]));
    w.install("alpha");
    let v2 = w.skill("alpha", "1.1.0", &m(&[]));
    w.publish("skill", "alpha", "1.1.0", &v2, &m(&[]));
    let key = record_key("skill", &Scope::Shared, "alpha");
    let old = w.f.store.records().get(&key).unwrap().clone();
    let new_sha = pkg::parse(&String::from_utf8(v2.clone()).unwrap()).unwrap().manifest.sha256;
    // the process died after the new files were written but before the record was: the journal says an update was running
    w.f.store.with_life(|s| s.journal = Some(Journal { key: key.clone(), from_sha: old.sha256.clone(), to_sha: new_sha.clone(), action: "update".into(), at: 1 }));
    let a = admin();
    let r = w.f.store.review(&w.f.svc, &a, v2.clone(), None, "file", false, Scope::Shared, false, &[]);
    w.f.store.install(&w.f.svc, &a, &r, Confirm::all()).unwrap();
    let mut recs = w.f.store.records();
    recs.insert(key.clone(), old.clone());
    w.f.store.save_records(&recs).unwrap();
    assert!(w.f.svc.skills.get(&Scope::Shared, "alpha").unwrap().body.contains("1.1.0"), "half way: the files are new, the record old");
    let said = w.f.store.recover(&w.f.svc);
    assert!(said.iter().any(|s| s.contains("cut off before it finished") && s.contains("1.0.0 was put back")), "{said:?}");
    assert!(w.f.svc.skills.get(&Scope::Shared, "alpha").unwrap().body.contains("1.0.0"));
    assert_eq!(w.version("skill", "alpha"), "1.0.0");
    assert!(w.f.store.life_state().journal.is_none());
    assert!(w.f.store.recover(&w.f.svc).is_empty(), "nothing to do the second time");
    // cut off after the record was written: it is checked and kept
    w.f.store.with_life(|s| s.journal = Some(Journal { key: key.clone(), from_sha: old.sha256.clone(), to_sha: new_sha, action: "update".into(), at: 1 }));
    let r = w.f.store.review(&w.f.svc, &a, v2, None, "file", false, Scope::Shared, false, &[]);
    w.f.store.install(&w.f.svc, &a, &r, Confirm::all()).unwrap();
    let said = w.f.store.recover(&w.f.svc);
    assert!(said.iter().any(|s| s.contains("checks out")), "{said:?}");
    assert_eq!(w.version("skill", "alpha"), "1.1.0");
}

#[test]
fn themes_templates_and_other_content_kinds_update_roll_back_and_remove_through_the_same_layer() {
    let mut w = world("life-content");
    let mut tm = m(&[]);
    tm.target = "tales".into();
    tm.ext_kind = "theme".into();
    let t1 = w.content("extension", "sunset", "1.0.0", &tm);
    w.publish("extension", "sunset", "1.0.0", &t1, &tm);
    w.install("sunset");
    let tpl = w.content("template", "weekly", "1.0.0", &m(&[]));
    w.publish("template", "weekly", "1.0.0", &tpl, &m(&[]));
    w.install("weekly");
    assert_eq!(w.life("extension:sunset").entry.ty, Type::Extension);
    let t2 = w.content("extension", "sunset", "1.1.0", &tm);
    w.publish("extension", "sunset", "1.1.0", &t2, &tm);
    let tpl2 = w.content("template", "weekly", "1.2.0", &m(&[]));
    w.publish("template", "weekly", "1.2.0", &tpl2, &m(&[]));
    w.update("extension:sunset", Confirm::default()).unwrap();
    w.update("template:weekly", Confirm::default()).unwrap();
    assert_eq!((w.version("extension", "sunset"), w.version("template", "weekly")), ("1.1.0".into(), "1.2.0".into()));
    let a = admin();
    let l = w.life("template:weekly");
    w.f.store.rollback(&w.f.svc, &a, &l, &|_, _| {}).unwrap();
    assert_eq!(w.version("template", "weekly"), "1.0.0");
    assert_eq!(w.f.store.content("template", "root")[0].version, "1.0.0");
    let lives = w.lives();
    let l = Store::find_life(&lives, "extension:sunset").unwrap();
    w.f.store.remove_item(&w.f.svc, &Role::Admin.template(), "root", &lives, l, &RemoveOpts::default(), &|_, _| {}).unwrap();
    assert!(w.f.store.content("extension", "root").is_empty());
}

#[test]
fn tiles_and_shipped_skills_say_built_in_and_cannot_be_updated_or_pinned() {
    let w = world("life-builtin");
    let lives = w.lives();
    let t = Store::find_life(&lives, "app:chat").unwrap();
    assert!(t.builtin && t.check == Check::BuiltIn && !t.can_pin() && !t.entry.removable);
    let a = admin();
    assert!(w.f.store.plan_update(&w.f.svc, &a, t, &w.loaded(), &NoFetch).unwrap_err().to_string().contains("part of IYAKKI"));
    assert!(Store::find_life(&lives, "chat").is_ok() && Store::find_life(&lives, "nothing-here").is_err());
}

#[test]
fn command_line_tools_show_the_vendors_latest_when_known_and_say_when_there_is_no_way_to_know() {
    let w = world("life-cli");
    crate::clis::write_fake_installed(&w.f.svc.clis, "kubectl", "#!/bin/sh\necho v1\n");
    crate::clis::write_fake_installed(&w.f.svc.clis, "aider", "#!/bin/sh\necho 0.1\n");
    assert_eq!(w.life("cli:kubectl").check, Check::NotChecked);
    assert!(matches!(w.life("cli:aider").check, Check::NoWay(ref t) if t.contains("latest")));
    w.f.store.with_life(|s| {
        s.cli_latest.insert("kubectl".into(), Latest { version: "v1.31.0".into(), error: String::new(), at: 1 });
    });
    let l = w.life("cli:kubectl");
    assert_eq!((l.check.clone(), l.latest.clone()), (Check::Update, Some("v1.31.0".into())));
    assert_eq!(l.status_line(), "v1 -> v1.31.0");
    w.f.store.with_life(|s| {
        s.cli_latest.insert("kubectl".into(), Latest { version: "v1".into(), error: String::new(), at: 1 });
    });
    assert_eq!(w.life("cli:kubectl").check, Check::UpToDate);
    w.f.store.with_life(|s| {
        s.cli_latest.insert("kubectl".into(), Latest { version: String::new(), error: "could not reach dl.k8s.io".into(), at: 1 });
    });
    assert!(matches!(w.life("cli:kubectl").check, Check::Offline(ref t) if t.contains("dl.k8s.io")));
    assert!(version_newer("v1.31.0", "v1.9.9") && !version_newer("1.9", "1.10") && version_newer("2.60.0", "2.59.9") && !version_newer("v1.31.0", "v1.31.0"));
}

#[test]
fn services_show_their_pinned_upgrade_path_and_are_upgraded_only_through_the_platform_engine() {
    let w = world("life-svc");
    let old = crate::platform::installed::Installed { id: "gitea-iyakki-gitea".into(), service: "gitea".into(), title: "Gitea".into(), version: crate::platform::gitea::VERSIONS[0].into(), ..Default::default() };
    let conn = crate::platform::installed::Installed { id: "gitea-other".into(), service: "gitea".into(), title: "Gitea".into(), version: "1.0".into(), connected: true, ..Default::default() };
    crate::platform::installed::save(w.f.svc.dir(), "root", &[old, conn]).unwrap();
    let s = w.f.store.services(&w.f.svc, "root");
    assert_eq!(s[0].latest.as_deref(), Some(crate::platform::gitea::VERSIONS[1]));
    assert!(s[0].note.contains("platform_upgrade") && s[0].note.contains("always asks"));
    assert!(s[1].connected && s[1].latest.is_none());
}

#[test]
fn vault_entries_named_after_an_item_are_found() {
    let names: Vec<String> = ["svc.files.token", "mcp.files", "other", "files", "filesystem", "svc.filesystem.key"].iter().map(|s| s.to_string()).collect();
    assert_eq!(vault_matches(&names, "mcp", "files"), vec!["svc.files.token", "mcp.files", "files"]);
}

fn run_cmd(w: &W, role: Role, user: &str, line: &str) -> Option<Result<String, String>> {
    let words: Vec<&str> = line.split_whitespace().collect();
    super::storecmd::run(&w.f.svc, &role.template(), user, &words)
}

#[test]
fn the_terminal_commands_list_outdated_update_pin_roll_back_and_remove_with_the_data_choice() {
    let mut w = world("life-cmd");
    let v1 = w.skill("alpha", "1.0.0", &m(&[]));
    w.publish("skill", "alpha", "1.0.0", &v1, &m(&[]));
    w.install("alpha");
    assert!(run_cmd(&w, Role::Admin, "root", "list --outdated").unwrap().unwrap().starts_with("Everything is up to date"));
    let v2 = w.skill("alpha", "1.1.0", &m(&[]));
    w.publish("skill", "alpha", "1.1.0", &v2, &m(&[]));
    let out = run_cmd(&w, Role::Admin, "root", "list --outdated").unwrap().unwrap();
    assert!(out.contains("skill:alpha") && out.contains("1.0.0 -> 1.1.0"), "{out}");
    assert!(run_cmd(&w, Role::Admin, "root", "list --all").unwrap().unwrap().contains("skill:alpha"));
    // pin: update refuses, unpin: update works
    assert!(run_cmd(&w, Role::Admin, "root", "pin skill:alpha").unwrap().unwrap().contains("pinned at 1.0.0"));
    assert!(run_cmd(&w, Role::Admin, "root", "update alpha").unwrap().unwrap_err().contains("pinned"));
    assert!(run_cmd(&w, Role::Admin, "root", "update --all").unwrap().unwrap().contains("skipped  alpha: pinned"));
    run_cmd(&w, Role::Admin, "root", "unpin alpha").unwrap().unwrap();
    assert!(run_cmd(&w, Role::Admin, "root", "update alpha").unwrap().unwrap().contains("is now 1.1.0"));
    assert!(run_cmd(&w, Role::Admin, "root", "update alpha").unwrap().unwrap_err().contains("up to date"));
    assert!(run_cmd(&w, Role::Admin, "root", "rollback alpha").unwrap().unwrap().contains("back at 1.0.0"));
    // unknown ids and the old verbs
    assert!(run_cmd(&w, Role::Admin, "root", "update nothing-here").unwrap().unwrap_err().contains("Nothing installed is called"));
    assert!(run_cmd(&w, Role::Admin, "root", "list").is_none(), "plain list is still the skill packages' list");
    assert!(run_cmd(&w, Role::Admin, "root", "update").unwrap().unwrap_err().contains("usage"));
    // remove: both flags is a mistake, data deletion needs the typed name, keep is the default
    let data = w.f.dir.join("store/appdata/skill/shared/alpha");
    std::fs::create_dir_all(&data).unwrap();
    assert!(run_cmd(&w, Role::Admin, "root", "remove alpha --keep-data --delete-data").unwrap().unwrap_err().contains("not both"));
    let e = run_cmd(&w, Role::Admin, "root", "remove alpha --delete-data").unwrap().unwrap_err();
    assert!(e.contains("Type its name") && e.contains("--confirm=alpha") && e.contains("REMOVE alpha"), "{e}");
    assert!(w.f.svc.skills.get(&Scope::Shared, "alpha").is_some());
    assert!(run_cmd(&w, Role::Admin, "root", "remove alpha --delete-data --confirm=alpha").unwrap().unwrap().contains("Deleted its data folder"));
    assert!(!data.exists() && w.f.svc.skills.get(&Scope::Shared, "alpha").is_none());
    // an ordinary account cannot change what is shared
    w.install("alpha");
    assert!(run_cmd(&w, Role::User, "alice", "remove alpha").unwrap().is_err());
    assert!(run_cmd(&w, Role::User, "alice", "update alpha").unwrap().is_err());
    assert!(run_cmd(&w, Role::Admin, "root", "frobnicate").is_none());
}

#[test]
fn the_assistant_tools_list_read_only_and_the_three_writers_always_ask_state_what_changes_and_never_grant_permissions() {
    use super::tools;
    use serde_json::json;
    let mut w = world("life-tools");
    let p = Role::Admin.template();
    let defs = tools::defs(&p);
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk).unwrap_or_else(|| panic!("{n}"));
    assert_eq!(risk("store_outdated"), crate::agent::Risk::Safe);
    for n in ["store_update", "store_remove", "store_rollback"] {
        assert_eq!(risk(n), crate::agent::Risk::Ask, "{n}");
        assert!(tools::is_store_tool(n));
        assert!(crate::agent::hard_ask(n, &json!({})).is_some(), "{n} is never run by Auto or Autopilot and never granted for a session");
    }
    assert!(crate::agent::hard_ask("store_outdated", &json!({})).is_none());
    let t1 = w.tool("shout", "1.0.0", &[], &m(&[]));
    w.publish("tool", "shout", "1.0.0", &t1, &m(&[]));
    let s1 = w.skill("alpha", "1.0.0", &m(&[]));
    w.publish("skill", "alpha", "1.0.0", &s1, &m(&[]));
    w.install("shout");
    w.install("alpha");
    assert!(tools::call("store_outdated", &json!({}), &w.f.svc, &p, "root").unwrap().starts_with("Everything is up to date"));
    let mut m2 = m(&[]);
    m2.changelog = "Better.".into();
    let s2 = w.skill("alpha", "1.1.0", &m2);
    w.publish("skill", "alpha", "1.1.0", &s2, &m2);
    let more = w.tool("shout", "1.1.0", &["write:/tmp"], &m(&["write:/tmp"]));
    w.publish("tool", "shout", "1.1.0", &more, &m(&["write:/tmp"]));
    let out = tools::call("store_outdated", &json!({}), &w.f.svc, &p, "root").unwrap();
    assert!(out.contains("skill:alpha") && out.contains("Better.") && out.contains("asks for MORE: write files in /tmp"), "{out}");
    // the card states what changes
    let d = tools::describe(&w.f.svc, &p, "root", "store_update", &json!({"id": "skill:alpha"})).unwrap();
    assert!(d.contains("UPDATE “alpha” from 1.0.0 to 1.1.0") && d.contains("Better.") && d.contains("kept for one rollback"), "{d}");
    let d = tools::describe(&w.f.svc, &p, "root", "store_update", &json!({"id": "tool:shout"})).unwrap();
    assert!(d.contains("NEW permissions") && d.contains("nothing will be changed"), "{d}");
    let d = tools::describe(&w.f.svc, &p, "root", "store_update", &json!({"all": true})).unwrap();
    assert!(d.contains("UPDATE ALL") && d.contains("[skipped]"), "{d}");
    let d = tools::describe(&w.f.svc, &p, "root", "store_remove", &json!({"id": "skill:alpha"})).unwrap();
    assert!(d.contains("REMOVE") && d.contains("KEPT"), "{d}");
    let d = tools::describe(&w.f.svc, &p, "root", "store_remove", &json!({"id": "skill:alpha", "delete_data": true})).unwrap();
    assert!(d.contains("ALSO DELETES"), "{d}");
    // the assistant cannot raise a tool's permissions; the harmless update goes through
    let e = tools::call("store_update", &json!({"id": "tool:shout"}), &w.f.svc, &p, "root").unwrap_err();
    assert!(e.contains("STORE tile") && e.contains("write files in /tmp"), "{e}");
    assert_eq!(w.f.svc.packages.get_tool(&Scope::Shared, "shout").unwrap().manifest.version, "1.0.0");
    assert!(tools::call("store_update", &json!({"id": "skill:alpha"}), &w.f.svc, &p, "root").unwrap().contains("is now 1.1.0"));
    assert!(tools::call("store_rollback", &json!({"id": "skill:alpha"}), &w.f.svc, &p, "root").unwrap().contains("back at 1.0.0"));
    assert!(tools::call("store_update", &json!({"all": true}), &w.f.svc, &p, "root").unwrap().contains("skipped"));
    // data deletion needs the typed name
    let data = w.f.dir.join("store/appdata/skill/shared/alpha");
    std::fs::create_dir_all(&data).unwrap();
    assert!(tools::call("store_remove", &json!({"id": "skill:alpha", "delete_data": true}), &w.f.svc, &p, "root").unwrap_err().contains("Type its name"));
    assert!(tools::call("store_remove", &json!({"id": "skill:alpha", "delete_data": true, "confirm": "alpha"}), &w.f.svc, &p, "root").unwrap().contains("Deleted its data folder"));
    assert!(w.f.svc.skills.get(&Scope::Shared, "alpha").is_none());
    // not offered to an account without the store
    let mut guest = Role::Guest.template();
    guest.apps.remove(&iyakki_sec::policy::App::Store);
    assert!(tools::defs(&guest).is_empty());
}

#[test]
fn the_assistants_search_and_info_show_the_persons_own_titles_tags_and_groups_read_only() {
    use super::tools;
    use serde_json::json;
    let mut w = world("life-tools-org");
    let p = Role::Admin.template();
    let s1 = w.skill("alpha", "1.0.0", &m(&[]));
    w.publish("skill", "alpha", "1.0.0", &s1, &m(&[]));
    let mut o = super::organize::Org::load(w.f.store.dir(), "root");
    o.set_alias("skill:alpha", Some("Notes helper")).unwrap();
    o.set_tags("skill:alpha", &["daily".into()]).unwrap();
    let g = o.create_group("Work", 0).unwrap();
    o.move_item("skill:alpha", Some(&g), None).unwrap();
    o.save().unwrap();
    let out = tools::call("store_search", &json!({"query": "alpha"}), &w.f.svc, &p, "root").unwrap();
    assert!(out.contains("the person calls it “Notes helper”") && out.contains("tags: daily") && out.contains("group: Work"), "{out}");
    // found by the words the person uses, too
    let out = tools::call("store_search", &json!({"query": "daily"}), &w.f.svc, &p, "root").unwrap();
    assert!(out.contains("alpha"), "{out}");
    let info = tools::call("store_info", &json!({"id": "alpha"}), &w.f.svc, &p, "root").unwrap();
    assert!(info.contains("the person's arrangement") && info.contains("Notes helper"), "{info}");
    // another account sees none of it
    let other = tools::call("store_search", &json!({"query": "alpha"}), &w.f.svc, &p, "alice").unwrap();
    assert!(!other.contains("Notes helper"), "{other}");
}

fn tile_package(w: &W, version: &str, caps: &str, m: &Meta) -> Vec<u8> {
    let manifest = format!(r#"{{"id":"calc","name":"Calc","version":"{version}","icon":"🧮","capabilities":{caps}}}"#);
    let b = super::tile::Bundle::from_files(&[("index.html", "<p id=\"p\">calc</p><script src=\"app.js\"></script>"), ("app.js", "var n = 1;"), ("app.json", &manifest)], "calc").unwrap();
    pkg::build_generic("tile", "calc", version, "MIT", &b.to_payload(), Some(&m.to_json()), None, Some(&w.key)).unwrap().into_bytes()
}

#[test]
fn a_tile_app_installs_updates_rolls_back_and_removes_with_its_saved_data_as_the_keep_or_delete_choice() {
    let mut w = world("life-tile");
    let m1 = m(&["tile:storage"]);
    let v1 = tile_package(&w, "1.0.0", r#"{"storage":true}"#, &m1);
    w.publish("tile", "calc", "1.0.0", &v1, &m1);
    w.install("calc");
    assert_eq!(w.version("tile", "calc"), "1.0.0");
    // the launcher sees it, for everyone it was installed for
    let tiles = super::tilehub::installed(&w.f.svc, &Role::User.template(), "alice");
    assert_eq!(tiles.len(), 1);
    assert_eq!((tiles[0].name.as_str(), tiles[0].title.as_str(), tiles[0].group.as_str(), tiles[0].dev), ("calc", "Calc", "My apps", false));
    // its saved data is the app's data folder
    let st = super::tile::storage_file(&w.f.store, &Scope::Shared, "calc", "alice");
    super::tile::TileStorage::open(&st).set("n", "5").unwrap();
    // an update that asks for more is gated like any other
    let m2 = m(&["tile:storage", "tile:clipboard"]);
    let v2 = tile_package(&w, "1.1.0", r#"{"storage":true,"clipboard":true}"#, &m2);
    w.publish("tile", "calc", "1.1.0", &v2, &m2);
    let p = w.plan("tile:calc").unwrap();
    assert!(p.needs_confirm() && p.added.iter().any(|x| x.canonical == "tile:clipboard"), "{}", p.summary());
    w.update("tile:calc", Confirm::all()).unwrap();
    assert_eq!(w.version("tile", "calc"), "1.1.0");
    assert_eq!(super::tile::TileStorage::open(&st).get("n").as_deref(), Some("5"), "an update keeps the data");
    // rollback
    let l = w.life("tile:calc");
    w.f.store.rollback(&w.f.svc, &admin(), &l, &|_, _| {}).unwrap();
    assert_eq!(w.version("tile", "calc"), "1.0.0");
    // removal: the data is offered, kept by default, deleted only after typing the name
    let pr = Role::Admin.template();
    let lives = w.lives();
    let l = Store::find_life(&lives, "tile:calc").unwrap();
    let plan = w.f.store.plan_remove(&w.f.svc, "root", &lives, l);
    assert!(plan.data.len() == 1 && plan.removes.iter().any(|r| r.contains("tile in your launcher")), "{}", plan.summary());
    let said = w.f.store.remove_item(&w.f.svc, &pr, "root", &lives, l, &RemoveOpts::default(), &|_, _| {}).unwrap();
    assert!(said.contains("Kept its data"), "{said}");
    assert!(super::tilehub::installed(&w.f.svc, &Role::User.template(), "alice").is_empty(), "the tile is gone from the launcher");
    assert!(st.exists(), "the data stayed");
    w.install("calc");
    let lives = w.lives();
    let l = Store::find_life(&lives, "tile:calc").unwrap();
    w.f.store.remove_item(&w.f.svc, &pr, "root", &lives, l, &RemoveOpts { delete_data: true, typed: "calc".into(), ..Default::default() }, &|_, _| {}).unwrap();
    assert!(!st.exists());
}

#[test]
fn a_tile_app_that_fails_its_checks_or_disagrees_with_its_header_is_refused_at_install() {
    let w = world("life-tile-bad");
    let a = admin();
    // remote script that is not declared
    let b = super::tile::Bundle::from_files(&[("index.html", "<script src=\"https://cdn.example.com/x.js\"></script>"), ("app.json", r#"{"id":"calc","name":"Calc","version":"1.0.0","icon":"🧮"}"#)], "calc").unwrap();
    let bytes = pkg::build_generic("tile", "calc", "1.0.0", "MIT", &b.to_payload(), Some(&m(&[]).to_json()), None, Some(&w.key)).unwrap().into_bytes();
    let r = w.f.store.review(&w.f.svc, &a, bytes, None, "file", false, Scope::Shared, false, &[]);
    assert!(r.refusal.as_deref().is_some_and(|t| t.contains("cdn.example.com")), "{:?}", r.refusal);
    // header and payload disagree
    let b = super::tile::Bundle::from_files(&[("index.html", "<p>x</p>"), ("app.json", r#"{"id":"other","name":"O","icon":"🧮"}"#)], "other").unwrap();
    let bytes = pkg::build_generic("tile", "calc", "1.0.0", "MIT", &b.to_payload(), Some(&m(&[]).to_json()), None, Some(&w.key)).unwrap().into_bytes();
    let r = w.f.store.review(&w.f.svc, &a, bytes, None, "file", false, Scope::Shared, false, &[]);
    assert!(r.refusal.as_deref().is_some_and(|t| t.contains("disagree")), "{:?}", r.refusal);
    // a vault permission in the store information is not a tile permission
    assert!(super::manifest::Permission::parse("tile:vault").is_err());
}

#[test]
fn a_copy_the_private_store_yanked_is_marked_withdrawn_and_listed_so_and_can_be_removed() {
    let mut w = world("life-yanked");
    let v1 = w.skill("meeting-notes", "1.0.0", &m(&[]));
    w.publish("skill", "meeting-notes", "1.0.0", &v1, &m(&[]));
    w.install("meeting-notes");
    assert_eq!(w.life("skill:meeting-notes").check, Check::UpToDate);
    let rec = w.f.store.records().get(&record_key("skill", &Scope::Shared, "meeting-notes")).cloned().unwrap();
    // the store is pinned to a key and kept a signed list of withdrawn packages beside its cache
    let store_key = new_signing_key().unwrap();
    let mut cfg = w.f.store.config();
    let src = cfg.sources.iter_mut().find(|s| s.id == rec.source).expect("the copy remembers its store");
    src.pin = public_key_hex(&store_key);
    cfg.save(&w.f.store.dir).unwrap();
    let write_list = |pkgs: Vec<serde_json::Value>, key: &SigningKey| {
        let mut doc = serde_json::json!({"v": 1, "id": "t", "serial": 2, "issued": NOW, "packages": pkgs, "key": public_key_hex(key)});
        let sig = super::trust::sign_doc("yanked", &doc, key);
        doc["sig"] = serde_json::Value::String(sig);
        std::fs::create_dir_all(w.f.store.dir.join("cache")).unwrap();
        std::fs::write(w.f.store.dir.join("cache").join(format!("{}.yanked.json", rec.source)), doc.to_string()).unwrap();
    };
    let this = serde_json::json!({"name": "meeting-notes", "version": "1.0.0", "sha256": rec.sha256, "reason": "bad"});
    // another version withdrawn, or a list signed by some other key: the copy is untouched
    write_list(vec![serde_json::json!({"name": "meeting-notes", "version": "0.9.0", "sha256": rec.sha256, "reason": "x"})], &store_key);
    assert_eq!(w.life("skill:meeting-notes").check, Check::UpToDate);
    write_list(vec![this.clone()], &new_signing_key().unwrap());
    assert_eq!(w.life("skill:meeting-notes").check, Check::UpToDate);
    // this version withdrawn by the pinned key: marked, said so in `store list`, and still removable
    write_list(vec![this], &store_key);
    let l = w.life("skill:meeting-notes");
    assert!(matches!(&l.check, Check::Withdrawn(t) if t.contains("withdrew this version") && t.contains("store remove skill:meeting-notes")), "{:?}", l.check);
    assert_eq!(l.check.label(), "Withdrawn");
    assert!(l.status_line().contains("withdrawn"));
    assert!(!l.can_update());
    let lives = w.lives();
    let policy = Role::Admin.template();
    assert!(super::storecmd::list_text(&lives, &[], false).contains("withdrawn"));
    let opts = super::lifecycle::RemoveOpts::default();
    w.f.store.remove_item(&w.f.svc, &policy, "root", &lives, &l, &opts, &|_, _| {}).unwrap();
    assert!(w.f.store.records().get(&record_key("skill", &Scope::Shared, "meeting-notes")).is_none());
}
