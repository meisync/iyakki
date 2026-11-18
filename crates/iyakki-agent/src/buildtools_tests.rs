//! The shared-build chat tools with a fake backend (and a fake streamer for the file download): what is read-only, what asks and never gets
//! a session grant, that a card names the folder and the size, that nothing is installed and that a tampered file is refused.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Risk, Tools};
use crate::buildtools::*;
use crate::store::buildshare::{BFile, Build, List};

struct Fake {
    cfg: Mutex<Option<Cfg>>,
    list: Result<List, String>,
    fetched: Mutex<Vec<String>>,
    held: Option<std::collections::BTreeSet<String>>,
    installed: Mutex<Vec<String>>,
}

impl Backend for Fake {
    fn config(&self) -> Option<Cfg> {
        self.cfg.lock().unwrap().clone()
    }
    fn set_config(&self, c: &Cfg) -> Result<(), String> {
        *self.cfg.lock().unwrap() = Some(c.clone());
        Ok(())
    }
    fn list(&self, _: &Cfg) -> Result<List, String> {
        self.list.clone()
    }
    fn fetch(&self, _: &Cfg, b: &Build, dir: &std::path::Path) -> Result<String, String> {
        self.fetched.lock().unwrap().push(format!("{} -> {}", b.build_id, dir.display()));
        Ok(format!("Fetched build {}.", b.build_id))
    }
    fn out_root(&self) -> std::path::PathBuf {
        "/data/local/builds".into()
    }
    fn held_layers(&self) -> Option<std::collections::BTreeSet<String>> {
        self.held.clone()
    }
    fn install_preview(&self, dir: &std::path::Path) -> Result<Vec<String>, String> {
        Ok(vec![format!("IYAKKI 0.1.0 -> 0.2.0 from {}", dir.display()), "Needs your yes: no signature".into()])
    }
    fn install(&self, dir: &std::path::Path, who: &str) -> Result<String, String> {
        self.installed.lock().unwrap().push(format!("{} by {who}", dir.display()));
        Ok("Installed. Restart when you are ready.".into())
    }
}

const KEY: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";

fn rig(configured: bool) -> (BuildTools, Arc<Fake>) {
    let b = Build { variant: "pc-x86_64".into(), channel: "dev".into(), build_id: "alpha4b".into(), version: "0.1.0-alpha.4".into(), pack_variant: "pc".into(), commit: "0123456789abcdef".into(), published: 1, seq: 1, total: 420_000_000, files: vec![BFile { path: "update.json".into(), sha256: "a".repeat(64), size: 10 }], needs: vec![] };
    let f = Arc::new(Fake { cfg: Mutex::new(configured.then(|| Cfg { url: "http://10.0.0.5:30580".into(), key: KEY.into(), token_file: String::new() })), list: Ok(List { builds: vec![b], ..Default::default() }), fetched: Mutex::new(vec![]), held: None, installed: Mutex::new(vec![]) });
    (BuildTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn status_is_read_only_and_names_the_newest_build_per_machine_type() {
    let (t, f) = rig(true);
    let d = t.defs();
    assert_eq!(d.iter().find(|d| d.spec.name == STATUS_TOOL).unwrap().risk, Risk::Safe);
    assert_eq!(d.iter().find(|d| d.spec.name == FETCH_TOOL).unwrap().risk, Risk::Ask);
    assert_eq!(d.iter().find(|d| d.spec.name == SET_TOOL).unwrap().risk, Risk::Ask);
    let out = t.call(STATUS_TOOL, &json!({})).unwrap();
    assert!(out.contains("pc-x86_64 / dev: alpha4b") && out.contains("only the newest"), "{out}");
    assert!(f.fetched.lock().unwrap().is_empty());
    assert!(t.approved_always(STATUS_TOOL) && !t.approved_always(FETCH_TOOL));
}

#[test]
fn fetch_and_set_always_ask_and_the_card_names_the_folder_and_the_size() {
    let (t, f) = rig(true);
    for n in [FETCH_TOOL, SET_TOOL] {
        assert!(t.grant_blocker(n, &json!({})).is_some(), "{n} never gets a session grant");
        assert!(!t.pre_approved(n));
    }
    let card = t.preview(FETCH_TOOL, &json!({"machine_type": "pc-x86_64"})).unwrap();
    assert!(card.contains("/data/local/builds/pc-x86_64-dev") && card.contains("Nothing is installed") && card.contains("alpha4b"), "{card}");
    assert!(f.fetched.lock().unwrap().is_empty(), "showing the card downloads nothing");
    let out = t.call(FETCH_TOOL, &json!({"machine_type": "pc-x86_64"})).unwrap();
    assert!(out.contains("iyakki dev update /data/local/builds/pc-x86_64-dev") && out.contains("Nothing was installed"), "{out}");
    assert_eq!(f.fetched.lock().unwrap().len(), 1);
}

#[test]
fn bad_input_is_refused_and_a_missing_store_says_what_to_do() {
    let (t, _) = rig(false);
    assert!(t.call(STATUS_TOOL, &json!({})).unwrap_err().contains("builds_store_set"));
    assert!(t.refusal(FETCH_TOOL, &json!({"machine_type": "../x"})).is_some());
    assert!(t.refusal(SET_TOOL, &json!({"url": "ftp://x", "update_public_key": KEY})).is_some());
    assert!(t.refusal(SET_TOOL, &json!({"url": "http://x", "update_public_key": "short"})).is_some());
    assert!(t.refusal(SET_TOOL, &json!({"url": "http://x:1", "update_public_key": KEY})).is_none());
    t.call(SET_TOOL, &json!({"url": "http://10.0.0.5:30580/", "update_public_key": KEY.to_uppercase()})).unwrap();
    assert!(t.call(STATUS_TOOL, &json!({})).is_ok(), "the store was remembered (trailing slash and key case normalised)");
}

struct Mem(Vec<u8>, u16);
impl iyakki_proxy::Streamer for Mem {
    fn open(&self, _: &str, h: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        assert!(h.iter().all(|(k, _)| k == "Authorization"));
        Ok(iyakki_proxy::StreamHead { status: self.1, headers: vec![], body: Box::new(std::io::Cursor::new(self.0.clone())) })
    }
}

#[test]
fn a_file_is_kept_only_when_its_size_and_hash_match_the_signed_list() {
    use sha2::{Digest, Sha256};
    let dir = crate::testdir::outside_project("buildtools", "dl");
    let data = b"some build bytes".to_vec();
    let sha: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
    let dest = dir.join("boot/ROOT.SFS");
    download_file(&Mem(data.clone(), 200), "http://x", &[], &sha, data.len() as u64, &dest).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    let bad = dir.join("boot/BAD");
    let mut other = data.clone();
    other[0] ^= 1;
    assert!(download_file(&Mem(other, 200), "http://x", &[], &sha, data.len() as u64, &bad).unwrap_err().contains("does not match"));
    assert!(!bad.exists() && !bad.with_extension("part").exists(), "a refused file leaves nothing behind");
    assert!(download_file(&Mem(data.clone(), 200), "http://x", &[], &sha, 3, &bad).unwrap_err().contains("larger"));
    assert!(download_file(&Mem(vec![], 404), "http://x", &[], &sha, 3, &bad).unwrap_err().contains("replaced"));
    assert!(download_file(&Mem(vec![], 401), "http://x", &[], &sha, 3, &bad).unwrap_err().contains("token"));
}

fn layered_rig(held: Option<Vec<&str>>) -> (BuildTools, Arc<Fake>) {
    let mk = |id: &str, seq: u64, needs: Vec<String>| Build { variant: "pc-x86_64".into(), channel: "dev".into(), build_id: id.into(), version: "0.2.0".into(), pack_variant: "pc".into(), commit: "c0ffee".into(), published: 1, seq, total: 10, files: vec![BFile { path: "update.json".into(), sha256: "a".repeat(64), size: 10 }], needs };
    let list = List { builds: vec![mk("full1", 1, vec![]), mk("layered2", 2, vec!["b".repeat(64)])], ..Default::default() };
    let f = Arc::new(Fake { cfg: Mutex::new(Some(Cfg { url: "http://10.0.0.5:30580".into(), key: KEY.into(), token_file: String::new() })), list: Ok(list), fetched: Mutex::new(vec![]), held: held.map(|h| h.iter().map(|c| c.repeat(64)).collect()), installed: Mutex::new(vec![]) });
    (BuildTools::new(Arc::new(crate::assign::tests_support::NoTools), f.clone()), f)
}

#[test]
fn install_asks_every_time_fetches_the_build_that_fits_this_device_and_installs_it_through_the_devices_own_path() {
    let (t, f) = layered_rig(Some(vec!["b"]));
    assert_eq!(t.defs().iter().find(|d| d.spec.name == INSTALL_TOOL).unwrap().risk, Risk::Ask);
    assert!(t.grant_blocker(INSTALL_TOOL, &json!({})).is_some() && !t.pre_approved(INSTALL_TOOL) && !t.approved_always(INSTALL_TOOL));
    let card = t.preview(INSTALL_TOOL, &json!({"machine_type": "pc-x86_64"})).unwrap();
    assert!(card.contains("layered2") && card.contains("INSTALLED") && card.contains("Needs your yes: no signature"), "{card}");
    assert!(f.fetched.lock().unwrap().is_empty() && f.installed.lock().unwrap().is_empty(), "the card does nothing");
    let out = t.call(INSTALL_TOOL, &json!({"machine_type": "pc-x86_64"})).unwrap();
    assert!(out.contains("Fetched build layered2") && out.contains("Installed."), "{out}");
    assert_eq!(f.installed.lock().unwrap().as_slice(), ["/data/local/builds/pc-x86_64-dev by assistant"]);
}

#[test]
fn a_device_without_the_base_layer_gets_the_full_build_and_a_single_root_device_too() {
    for held in [Some(vec!["c"]), None] {
        let (t, f) = layered_rig(held);
        let out = t.call(FETCH_TOOL, &json!({"machine_type": "pc-x86_64"})).unwrap();
        assert!(out.contains("Fetched build full1"), "{out}");
        assert_eq!(f.fetched.lock().unwrap().len(), 1);
        assert!(f.installed.lock().unwrap().is_empty(), "fetch installs nothing");
    }
}

#[test]
fn a_backend_that_cannot_install_says_so_and_nothing_is_fetched_as_installed() {
    struct NoInstall(Fake);
    impl Backend for NoInstall {
        fn config(&self) -> Option<Cfg> { self.0.config() }
        fn set_config(&self, c: &Cfg) -> Result<(), String> { self.0.set_config(c) }
        fn list(&self, c: &Cfg) -> Result<List, String> { self.0.list(c) }
        fn fetch(&self, c: &Cfg, b: &Build, d: &std::path::Path) -> Result<String, String> { self.0.fetch(c, b, d) }
        fn out_root(&self) -> std::path::PathBuf { self.0.out_root() }
    }
    let (_, f) = layered_rig(None);
    let inner = Arc::try_unwrap(f).ok().unwrap();
    let t = BuildTools::new(Arc::new(crate::assign::tests_support::NoTools), Arc::new(NoInstall(inner)));
    assert!(t.call(INSTALL_TOOL, &json!({"machine_type": "pc-x86_64"})).unwrap_err().contains("cannot install"));
}
