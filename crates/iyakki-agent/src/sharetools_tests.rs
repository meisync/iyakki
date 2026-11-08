//! The shared-folder chat tools over the real manager (nothing listens: start and stop are recorded by a wrapper): every write asks, the
//! card says what becomes reachable and by whom, the folder rule of the person's scope holds, and no password or link secret passes.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::sharedfolder::{Draft, Scope, ShareCfg, ShareInfo, Shares};
use crate::sharedsite::Env;
use crate::sharetools::*;

struct Rig {
    shares: Shares,
    running: Mutex<Vec<String>>,
    log: Mutex<Vec<String>>,
    vault: Vec<(String, String)>,
    scope: Scope,
}

impl Api for Rig {
    fn scope(&self) -> Scope {
        self.scope.clone()
    }
    fn list(&self) -> Vec<ShareInfo> {
        let run = self.running.lock().unwrap().clone();
        self.shares.list().into_iter().map(|mut i| { i.running = run.contains(&i.cfg.id); i }).collect()
    }
    fn check(&self, d: &Draft) -> Result<ShareCfg, String> {
        self.shares.check(d)
    }
    fn create(&self, d: &Draft) -> Result<String, String> {
        self.log.lock().unwrap().push(format!("create {} pw={}", d.name, !d.password.is_empty()));
        self.shares.create(d)
    }
    fn start(&self, id: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("start {id}"));
        self.running.lock().unwrap().push(id.into());
        Ok(())
    }
    fn stop(&self, id: &str) {
        self.log.lock().unwrap().push(format!("stop {id}"));
        self.running.lock().unwrap().retain(|x| x != id);
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("delete {id}"));
        self.shares.delete(id)
    }
    fn vault_password(&self, entry: &str) -> Result<String, String> {
        self.vault.iter().find(|(n, _)| n == entry).map(|(_, v)| v.clone()).ok_or_else(|| format!("no vault entry {entry}"))
    }
}

const PW: &str = "correct-horse-battery";

fn rig(tag: &str, admin: bool) -> (ShareTools, Arc<Rig>, PathBuf, PathBuf) {
    let root = crate::testdir::unique("sharetools", tag);
    let home = root.join("home/amy");
    let other = root.join("disk/photos");
    std::fs::create_dir_all(home.join("docs")).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let home = std::fs::canonicalize(&home).unwrap();
    let other = std::fs::canonicalize(&other).unwrap();
    let scope = if admin { Scope::Admin } else { Scope::Home(home.clone()) };
    let env = Env { clock: Arc::new(|| 1_000), audit: Arc::new(|_, _| {}), deny: vec![], free_space: Arc::new(|_| Some(1 << 30)) };
    let shares = Shares::open(&root.join("cfg/shares.json"), "amy", env, Arc::new(|_| true), scope.clone(), Arc::new(|| vec!["192.168.7.20".to_string()]));
    let r = Arc::new(Rig { shares, running: Mutex::new(vec![]), log: Mutex::new(vec![]), vault: vec![("svc.share-pw".into(), PW.into())], scope });
    (ShareTools::new(Arc::new(crate::assign::tests_support::NoTools), r.clone()), r, home, other)
}

#[test]
fn listing_is_free_and_every_write_asks_every_time_and_is_never_remembered() {
    let (t, _, _, _) = rig("risk", true);
    let defs = t.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).map(|d| d.risk);
    assert_eq!(risk(READ_TOOL), Some(Risk::Safe));
    for n in WRITE_TOOLS {
        assert_eq!(risk(n), Some(Risk::Ask), "{n}");
        assert!(!t.pre_approved(n) && !t.approved_always(n));
        assert!(t.grant_blocker(n, &json!({})).is_some());
        assert!(crate::agent::hard_ask(n, &json!({})).is_some(), "{n} is on the hard-ask list");
    }
    assert!(crate::agent::hard_ask(READ_TOOL, &json!({})).is_none());
}

#[test]
fn an_administrator_may_share_a_folder_of_a_disk_and_everyone_else_only_a_folder_of_their_own_home() {
    let (t, r, home, other) = rig("scope", false);
    let own = home.join("docs");
    let ok = t.preview("share_create", &json!({"name": "Docs", "folder": own, "access": "local"})).unwrap();
    assert!(ok.contains("inside your own home") && ok.contains("ONLY THIS COMPUTER"), "{ok}");
    let no = t.refusal("share_create", &json!({"name": "Photos", "folder": other})).unwrap();
    assert!(no.contains("own home folder") && no.contains("An administrator can share other folders"), "{no}");
    assert!(t.call("share_create", &json!({"name": "Photos", "folder": other})).is_err());
    assert!(r.log.lock().unwrap().is_empty(), "nothing was made");
    let (ta, ra, _, other_a) = rig("scope-admin", true);
    let c = ta.preview("share_create", &json!({"name": "Photos", "folder": other_a, "access": "local"})).unwrap();
    assert!(c.contains("any folder of the disks"), "{c}");
    ta.call("share_create", &json!({"name": "Photos", "folder": other_a, "access": "local"})).unwrap();
    assert_eq!(ra.shares.list().len(), 1);
    // the system stays out of reach even for an administrator
    assert!(ta.refusal("share_create", &json!({"name": "Sys", "folder": "/proc"})).is_some());
}

#[test]
fn the_card_says_what_becomes_reachable_by_whom_for_each_kind_of_access() {
    let (t, _, home, _) = rig("cards", true);
    let f = home.join("docs");
    let card = |access: &str, extra: serde_json::Value| {
        let mut a = json!({"name": "Docs", "folder": f, "access": access});
        for (k, v) in extra.as_object().unwrap() {
            a[k] = v.clone();
        }
        t.preview("share_create", &a).unwrap_or_else(|| t.refusal("share_create", &a).unwrap())
    };
    assert!(card("local", json!({})).contains("ONLY THIS COMPUTER"));
    let l = card("link", json!({"upload": true, "expires": "7 days"}));
    assert!(l.contains("secret link") && l.contains("UPLOAD") && l.contains("after 7 days") && l.contains("NOT started"), "{l}");
    let p = card("password", json!({"password_entry": "svc.share-pw"}));
    assert!(p.contains("knows the password") && p.contains("vault entry svc.share-pw") && p.contains("never sees it") && !p.contains(PW), "{p}");
    assert!(card("password", json!({})).contains("vault_request"), "a password share without an entry says how to get one");
    let o = card("open", json!({}));
    assert!(o.contains("EVERYONE ON THIS NETWORK"), "{o}");
    assert!(card("nonsense", json!({})).contains("access is"));
    assert!(card("link", json!({"password": PW})).contains("never send a password"));
}

#[test]
fn a_password_share_takes_the_password_from_the_vault_and_it_never_appears_in_a_result() {
    let (t, r, home, _) = rig("pw", true);
    let out = t.call("share_create", &json!({"name": "Docs", "folder": home.join("docs"), "access": "password", "password_entry": "svc.share-pw"})).unwrap();
    assert!(!out.contains(PW) && out.contains("not started"), "{out}");
    assert!(r.log.lock().unwrap().iter().any(|l| l == "create Docs pw=true"));
    let cfg = r.shares.list().remove(0).cfg;
    assert!(!cfg.pass_hash.is_empty() && !cfg.pass_hash.contains(PW));
    // a missing entry is refused without a share being made
    let e = t.call("share_create", &json!({"name": "Two", "folder": home.join("docs"), "access": "password", "password_entry": "svc.nope"})).unwrap_err();
    assert!(e.contains("no vault entry"), "{e}");
    assert_eq!(r.shares.list().len(), 1);
}

#[test]
fn start_stop_and_delete_have_their_cards_and_a_link_secret_is_never_printed() {
    let (t, r, home, _) = rig("life", true);
    t.call("share_create", &json!({"name": "Docs", "folder": home.join("docs")})).unwrap();
    let cfg = r.shares.list().remove(0).cfg;
    assert_eq!(cfg.token.len(), 32);
    let start = t.preview("share_start", &json!({"share": "docs"})).unwrap();
    assert!(start.contains("secret link") && start.contains("http://192.168.7.20:") && start.contains("/s/…/") && !start.contains(&cfg.token), "{start}");
    let out = t.call("share_start", &json!({"share": cfg.id})).unwrap();
    assert!(!out.contains(&cfg.token) && out.contains("Address:"), "{out}");
    let list = t.call("share_list", &json!({})).unwrap();
    assert!(list.contains("RUNNING") && list.contains("Docs") && !list.contains(&cfg.token), "{list}");
    assert!(t.refusal("share_start", &json!({"share": "Docs"})).unwrap().contains("already running"));
    assert!(t.preview("share_stop", &json!({"share": "Docs"})).unwrap().contains("kept"));
    t.call("share_stop", &json!({"share": "Docs"})).unwrap();
    assert!(t.call("share_list", &json!({})).unwrap().contains("stopped"));
    let del = t.preview("share_delete", &json!({"share": "Docs"})).unwrap();
    assert!(del.contains("DELETE") && del.contains("files are not touched"), "{del}");
    t.call("share_delete", &json!({"share": "Docs"})).unwrap();
    assert_eq!(t.call("share_list", &json!({})).unwrap(), "No shared folders yet.");
    assert!(t.refusal("share_stop", &json!({"share": "Docs"})).unwrap().contains("no such share"));
    assert!(t.refusal("share_stop", &json!({})).is_some());
}
