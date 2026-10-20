//! Developer mode on the device, without a network: switching on and off (administrators only, automatic switch-off), pairing (a good
//! code, a wrong one, five tries, expiry, a proof made for another certificate), revoking, and install / upgrade / reinstall / uninstall
//! through the real STORE pipeline in a temporary data folder, labelled Developer (local), never promoted, drivers refused, logged.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use iyakki_sec::policy::Role;

use super::ops::{self, Who};
use super::*;
use crate::store::install::tests::{admin, fx, user, Fx, NOW};
use crate::store::trust::Level;

fn clocked(f: &Fx) -> (DevMode, Arc<AtomicI64>) {
    let t = Arc::new(AtomicI64::new(NOW));
    let c = t.clone();
    (DevMode::open(&f.dir).with_clock(Arc::new(move || c.load(Ordering::SeqCst))), t)
}

fn tile_dir(f: &Fx, id: &str, version: &str, text: &str) -> PathBuf {
    let d = f.dir.join(format!("src-{id}"));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("app.json"), format!(r#"{{"id":"{id}","name":"{id} app","version":"{version}","group":"Dev","icon":"⏰"}}"#)).unwrap();
    std::fs::write(d.join("index.html"), format!("<!doctype html><html><head><title>{id}</title></head><body><h1>{text}</h1></body></html>")).unwrap();
    d
}

fn who() -> Who {
    Who { computer: "laptop".into(), account: "root".into() }
}

#[test]
fn only_an_administrator_switches_it_on_and_it_switches_itself_off_when_the_time_is_up() {
    let f = fx("dm-onoff");
    let (dm, t) = clocked(&f);
    assert!(!dm.is_on(), "off by default");
    assert!(dm.enable(&Role::User.template(), "bob", 0).is_err(), "not for users");
    assert!(dm.enable(&Role::Operator.template(), "op", 0).is_err(), "not for operators");
    assert!(dm.enable(&Role::Admin.template(), "root", MAX_AUTO_OFF_HOURS + 1).is_err());
    let st = dm.enable(&Role::Admin.template(), "root", 2).unwrap();
    assert_eq!((st.enabled, st.until, st.port()), (true, NOW + 7200, DEFAULT_PORT));
    assert!(dm.is_on() && is_on(&f.dir) == dm.is_on());
    assert!(dm.status_text().contains("switches off in 2 h"), "{}", dm.status_text());
    t.store(NOW + 7200, Ordering::SeqCst);
    assert!(!dm.is_on(), "switched off by itself");
    assert!(!dm.state().enabled, "and written down");
    assert!(dm.activity(5).iter().any(|a| a.action == "devmode.off" && a.detail.contains("automatically")));
    // the function the IYAKKI menu calls
    assert!(set_enabled(&f.dir, &Role::User.template(), "bob", true, 0).is_err());
    let d2 = DevMode::open(&f.dir);
    assert!(set_enabled(&f.dir, &Role::Admin.template(), "root", true, 0).unwrap().starts_with("On"));
    assert!(d2.is_on() && indicator(&f.dir).as_deref() == Some("DEV"));
    assert!(d2.disable(&Role::User.template(), "bob").is_err());
    assert_eq!(set_enabled(&f.dir, &Role::Admin.template(), "root", false, 0).unwrap(), "Off");
    assert!(indicator(&f.dir).is_none());
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn pairing_takes_the_right_code_once_counts_wrong_tries_expires_and_binds_the_certificate() {
    let f = fx("dm-pair");
    let (dm, t) = clocked(&f);
    let adm = Role::Admin.template();
    assert!(dm.start_pairing(&adm, "root").is_err(), "developer mode must be on first");
    dm.enable(&adm, "root", 0).unwrap();
    assert!(dm.start_pairing(&Role::User.template(), "bob").is_err(), "administrators open pairings");
    let p = dm.start_pairing(&adm, "root").unwrap();
    assert!(indicator(&f.dir).unwrap().contains("pairing"));
    let fp = "AB:CD:EF:01";
    let k = crate::pkg::new_signing_key().unwrap();
    let pubhex = crate::pkg::public_key_hex(&k);
    let proof = |code: &str, fp: &str| auth::pair_proof(&auth::pair_key(code, fp).unwrap(), fp, &pubhex, "laptop");
    // a wrong code, and the right code for another certificate (a machine in the middle), each use up a try
    assert!(dm.complete_pairing(fp, "laptop", &pubhex, &proof("AAAAA-AAAAA", fp)).unwrap_err().contains("4 tries left"));
    assert!(dm.complete_pairing(fp, "laptop", &pubhex, &proof(&p.code, "AB:CD:EF:02")).unwrap_err().contains("3 tries left"));
    assert!(dm.keys().is_empty());
    let (key, answer) = dm.complete_pairing(fp, "laptop", &pubhex, &proof(&p.code, fp)).unwrap();
    assert_eq!((key.name.as_str(), key.account.as_str(), key.key.as_str()), ("laptop", "root", pubhex.as_str()));
    assert_eq!(answer, auth::pair_answer(&auth::pair_key(&p.code, fp).unwrap(), fp, &pubhex), "the device proves it knows the code");
    assert!(dm.pairing().is_none(), "a code works once");
    assert!(dm.complete_pairing(fp, "laptop", &pubhex, &proof(&p.code, fp)).is_err());
    // five wrong tries close the pairing
    let p2 = dm.start_pairing(&adm, "root").unwrap();
    for _ in 0..PAIR_TRIES {
        let _ = dm.complete_pairing(fp, "other", &pubhex, &proof("BBBBB-BBBBB", fp));
    }
    assert!(dm.pairing().is_none());
    assert!(dm.complete_pairing(fp, "other", &pubhex, &proof(&p2.code, fp)).is_err(), "even the right code, after five wrong ones");
    // expiry
    let p3 = dm.start_pairing(&adm, "root").unwrap();
    t.store(NOW + PAIR_SECS + 1, Ordering::SeqCst);
    assert!(dm.complete_pairing(fp, "late", &pubhex, &proof(&p3.code, fp)).unwrap_err().contains("no pairing code"));
    // revoking
    assert!(dm.revoke(&Role::User.template(), "bob", "laptop").is_err());
    dm.revoke(&adm, "root", "laptop").unwrap();
    assert!(dm.find_key(&pubhex).is_none());
    let acts = dm.activity(50);
    assert!(acts.iter().any(|a| a.action == "devmode.pair" && a.ok) && acts.iter().any(|a| a.action == "devmode.pair" && !a.ok) && acts.iter().any(|a| a.action == "devmode.revoke"));
    assert!(acts.iter().all(|a| !a.detail.contains(&p.code)), "the code is never logged");
    // switching off closes an open pairing
    dm.start_pairing(&adm, "root").unwrap();
    dm.disable(&adm, "root").unwrap();
    assert!(dm.pairing().is_none());
    let _ = std::fs::remove_dir_all(&f.dir);
}

fn install_dir(f: &Fx, dm: &DevMode, actor: &crate::store::install::Actor, dir: &Path, key: &ed25519_dalek::SigningKey) -> Result<ops::Installed, String> {
    ops::install(&f.svc, &f.store, dm, actor, &who(), ops::package_bytes(dir, key).unwrap(), false)
}

#[test]
fn a_tile_installs_upgrades_and_reinstalls_through_the_store_as_developer_local_and_is_removed_again() {
    let f = fx("dm-install");
    let (dm, _) = clocked(&f);
    let key = crate::pkg::new_signing_key().unwrap();
    let a = admin();
    let dir = tile_dir(&f, "clock-dev", "1.0.0", "one");
    let r = install_dir(&f, &dm, &a, &dir, &key).unwrap();
    assert_eq!((r.kind.as_str(), r.name.as_str(), r.version.as_str(), r.level.as_str(), r.scope.as_str(), r.reinstalled), ("tile", "clock-dev", "1.0.0", "developer", "user/root", false));
    let rec = f.store.records().into_values().find(|x| x.name == "clock-dev").unwrap();
    assert_eq!((rec.source.as_str(), rec.level.as_str()), ("developer:laptop", Level::Developer.id()));
    assert!(f.store.content("tile", "root").iter().any(|c| c.name == "clock-dev"), "installed by the STORE's own pipeline");
    let tiles = crate::store::tilehub::installed(&f.svc, &Role::Admin.template(), "root");
    let t = tiles.iter().find(|e| e.name == "clock-dev").unwrap();
    assert!(t.is_developer() && t.trust == "Developer (local)", "labelled on the tile");
    // the same version again (a rebuild with other bytes) and a newer one
    std::fs::write(dir.join("index.html"), "<!doctype html><html><body><h1>two</h1></body></html>").unwrap();
    let stamp = reload_stamp(&f.dir, "clock-dev");
    assert!(install_dir(&f, &dm, &a, &dir, &key).unwrap().reinstalled);
    assert!(reload_stamp(&f.dir, "clock-dev") > stamp, "an open copy reloads");
    crate::store::publishflow::set_version(&dir, "1.1.0").unwrap();
    assert_eq!(install_dir(&f, &dm, &a, &dir, &key).unwrap().version, "1.1.0");
    // an older version is refused by the STORE's high-water rule
    crate::store::publishflow::set_version(&dir, "0.9.0").unwrap();
    assert!(install_dir(&f, &dm, &a, &dir, &key).unwrap_err().contains("older"));
    // never promoted: the lifecycle never looks for it in a store
    let lives = f.store.lifecycle(&f.svc, &Role::Admin.template(), "root", &f.store.local_catalogs(&f.svc), &Default::default());
    let l = lives.iter().find(|l| l.entry.name == "clock-dev").expect("listed");
    assert!(l.check.note().contains("developer computer"), "{:?}", l.check);
    // listing and removing
    assert_eq!(ops::list(&f.store, &a, false).len(), 1);
    assert!(ops::uninstall(&f.svc, &f.store, &dm, &a, &who(), "skill:clock-dev").is_err(), "kind must match");
    let gone = ops::uninstall(&f.svc, &f.store, &dm, &a, &who(), "clock-dev").unwrap();
    assert_eq!(gone.version, "1.1.0");
    assert!(f.store.content("tile", "root").is_empty() && ops::list(&f.store, &a, false).is_empty());
    let (log, next) = dm.read_app_log("clock-dev", 0);
    assert!(log.contains("installed tile:clock-dev 1.0.0") && log.contains("reinstalled") && log.contains("removed 1.1.0"), "{log}");
    assert_eq!(dm.read_app_log("clock-dev", next).0, "", "nothing new after the end");
    let acts = dm.activity(20);
    assert!(acts.iter().any(|x| x.action == "dev.install" && !x.ok && x.detail.contains("0.9.0")), "refusals are logged too");
    assert!(acts.iter().filter(|x| x.action == "dev.install" && x.ok).count() == 3);
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn roles_still_decide_and_store_installs_drivers_and_commands_are_out_of_reach() {
    let f = fx("dm-rules");
    let (dm, _) = clocked(&f);
    let key = crate::pkg::new_signing_key().unwrap();
    let dir = tile_dir(&f, "notes-dev", "1.0.0", "x");
    // an ordinary user may install for themselves, not for everyone
    let bob = user("bob");
    assert_eq!(install_dir(&f, &dm, &bob, &dir, &key).unwrap().scope, "user/bob");
    let shared = ops::install(&f.svc, &f.store, &dm, &bob, &who(), ops::package_bytes(&dir, &key).unwrap(), true);
    assert!(shared.unwrap_err().contains("for everyone"));
    // an MCP server: administrators, for everyone
    let mcp = crate::pkg::build_generic("mcp", "dev-files", "1.0.0", "MIT", r#"{"name":"dev-files","command":"/usr/bin/true","args":[]}"#, None, None, Some(&key)).unwrap().into_bytes();
    assert!(ops::install(&f.svc, &f.store, &dm, &bob, &who(), mcp.clone(), false).unwrap_err().contains("administrator"));
    assert_eq!(ops::install(&f.svc, &f.store, &dm, &admin(), &who(), mcp, false).unwrap().scope, "shared");
    // a driver pack is refused before the store sees it
    let meta = r#"{"title":"Wifi","ext_kind":"driver","target":"system"}"#;
    let drv = crate::pkg::build_generic("extension", "wifi-fw", "1.0.0", "MIT", r#"{"matches":[],"files":[]}"#, Some(meta), None, Some(&key)).unwrap().into_bytes();
    assert!(ops::install(&f.svc, &f.store, &dm, &admin(), &who(), drv, false).unwrap_err().contains("driver"));
    // a command-line tool or an `app` package is refused by the STORE's review
    let cli = crate::pkg::build_generic("cli", "kubectl", "1.0.0", "MIT", "{}", None, None, Some(&key)).unwrap().into_bytes();
    assert!(ops::install(&f.svc, &f.store, &dm, &admin(), &who(), cli, false).is_err());
    // something the STORE installed is not removed by developer mode
    let sk = crate::store::install::tests::skill_pkg("1.0.0", Some(&f.root), None, &crate::store::install::tests::meta("iyakki", &[]));
    let rv = f.store.review(&f.svc, &admin(), sk, None, "test", false, Scope::User("root".into()), false, &[]);
    f.store.install(&f.svc, &admin(), &rv, crate::store::install::Confirm::all()).unwrap();
    assert!(ops::uninstall(&f.svc, &f.store, &dm, &admin(), &who(), "meeting-notes").unwrap_err().contains("not installed from a developer computer"));
    // and a developer build cannot replace a store install signed by someone else (no silent take-over)
    let dev_skill = crate::store::install::tests::skill_pkg("2.0.0", Some(&key), None, &crate::store::install::tests::meta("iyakki", &[]));
    assert!(ops::install(&f.svc, &f.store, &dm, &admin(), &who(), dev_skill, false).unwrap_err().contains("different publisher"));
    assert_eq!(ops::list(&f.store, &admin(), true).iter().filter(|r| r.name == "meeting-notes").count(), 1);
    let _ = std::fs::remove_dir_all(&f.dir);
}

use crate::skills::Scope;
