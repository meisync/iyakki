use super::*;
use iyakki_shared::store_catalog::SourceCfg;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn key(n: u8) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[n; 32])
}

/// A store in memory: url to bytes.
struct Files(BTreeMap<String, Vec<u8>>);
impl Fetch for Files {
    fn get(&self, url: &str, max: usize) -> Result<Vec<u8>, String> {
        let b = self.0.get(url).ok_or_else(|| format!("{url} answered 404"))?;
        if b.len() > max {
            return Err("too big".into());
        }
        Ok(b.clone())
    }
}

fn store(version: &str, k: u8) -> StoreSource {
    let bytes = vec![3u8; 4000];
    let m = browserpkg::Manifest { version: version.into(), size: 4000, sha256: crate::pkg::hex(&Sha256::digest(&bytes)), arch: "x86_64".into() };
    let mut f = BTreeMap::new();
    f.insert("https://store.example/browser/x86_64/manifest.json".to_string(), browserpkg::manifest_doc(&m, &key(k)).to_string().into_bytes());
    f.insert("https://store.example/browser/x86_64/webview-helper".to_string(), bytes);
    StoreSource { fetch: Arc::new(Files(f)), base: "https://store.example".into(), arch: "x86_64".into() }
}

fn data(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-bpkg-tools-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn pin(n: u8) -> String {
    crate::pkg::public_key_hex(&key(n))
}

#[test]
fn the_three_tools_exist_only_for_accounts_with_the_store_and_the_browser_and_two_of_them_always_ask() {
    let all = iyakki_sec::policy::Role::Admin.template();
    let d = defs(&all);
    let names: Vec<_> = d.iter().map(|t| (t.spec.name.as_str(), t.risk)).collect();
    assert_eq!(names, vec![("browser_package_status", Risk::Safe), ("browser_package_install", Risk::Ask), ("browser_package_remove", Risk::Ask)]);
    assert!(d.iter().all(|t| is_package_tool(&t.spec.name)));
    let mut no_store = iyakki_sec::policy::Role::Admin.template();
    no_store.apps.remove(&App::Store);
    assert!(defs(&no_store).is_empty());
    for n in ["browser_package_install", "browser_package_remove"] {
        assert!(iyakki_shared::toolpolicy::hard_ask(n, &json!({})).is_some(), "{n} always asks");
    }
}

#[test]
fn install_from_a_pinned_store_then_status_then_remove_and_nothing_without_a_pinned_store() {
    let d = data("flow");
    let s = store("2.1.0", 1);
    assert!(run("browser_package_status", &json!({}), &d, None, "x86_64").unwrap().contains("not installed"));
    let card = run("browser_package_status", &json!({"check": true}), &d, Some((&s, &pin(1))), "x86_64").unwrap();
    assert!(card.contains("2.1.0") && card.contains("network and play sound"), "{card}");
    assert!(run("browser_package_status", &json!({"check": true}), &d, None, "x86_64").unwrap().contains("pinned key"));
    assert!(run("browser_package_install", &json!({}), &d, None, "x86_64").unwrap_err().contains("pinned key"));
    assert!(run("browser_package_install", &json!({}), &d, Some((&s, &pin(2))), "x86_64").unwrap_err().contains("not signed by the store key you trust"), "another key is refused");
    assert!(browserpkg::installed_version(&d).is_none(), "a refused install leaves nothing");
    assert!(run("browser_package_install", &json!({}), &d, Some((&s, &pin(1))), "x86_64").unwrap().contains("2.1.0"));
    assert!(run("browser_package_status", &json!({}), &d, None, "x86_64").unwrap().contains("2.1.0 is installed"));
    assert!(run("browser_package_remove", &json!({}), &d, None, "x86_64").unwrap().contains("removed"));
    assert!(browserpkg::installed_helper(&d).is_none());
    assert!(run("browser_package_remove", &json!({}), &d, None, "x86_64").unwrap().contains("nothing to remove"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_missing_file_in_the_store_is_a_plain_error_and_only_a_pinned_https_source_is_used() {
    let d = data("missing");
    let mut s = store("1.0.0", 1);
    s.base = "https://other.example".into();
    assert!(run("browser_package_install", &json!({}), &d, Some((&s, &pin(1))), "x86_64").unwrap_err().contains("404"));
    let src = |loc: &str, pin: &str, on: bool| SourceCfg { id: "a".into(), name: "a".into(), kind: SourceKind::Url, location: loc.into(), enabled: on, pin: pin.into() };
    let mut cfg = Config::default();
    assert!(pinned_source(&cfg).is_none());
    cfg.sources = vec![src("https://s.example/catalog.json", "", true), src("http://plain.example/catalog.json", "ab", true), src("https://off.example", "cd", false), src("https://good.example/catalog.json", "ef", true)];
    assert_eq!(pinned_source(&cfg), Some(("https://good.example".to_string(), "ef".to_string())));
    let _ = std::fs::remove_dir_all(&d);
}

/// A streaming transport in memory: url to bytes.
struct Streams(BTreeMap<String, Vec<u8>>);
impl iyakki_proxy::Streamer for Streams {
    fn open(&self, url: &str, _: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        match self.0.get(url) {
            Some(b) => Ok(iyakki_proxy::StreamHead { status: 200, headers: vec![], body: Box::new(std::io::Cursor::new(b.clone())) }),
            None => Ok(iyakki_proxy::StreamHead { status: 404, headers: vec![], body: Box::new(std::io::empty()) }),
        }
    }
}

fn streams(version: &str, k: u8) -> Arc<Streams> {
    let bytes = vec![5u8; 6000];
    let m = browserpkg::Manifest { version: version.into(), size: 6000, sha256: crate::pkg::hex(&Sha256::digest(&bytes)), arch: std::env::consts::ARCH.into() };
    let arch = std::env::consts::ARCH;
    let mut f = BTreeMap::new();
    f.insert(format!("https://store.example/browser/{arch}/manifest.json"), browserpkg::manifest_doc(&m, &key(k)).to_string().into_bytes());
    f.insert(format!("https://store.example/browser/{arch}/webview-helper"), bytes);
    Arc::new(Streams(f))
}

#[test]
fn the_stores_install_job_streams_the_package_from_the_pinned_store_and_only_from_it() {
    let d = data("job");
    // no store pinned: a plain refusal, nothing fetched, nothing kept
    let e = install_with_streamer(&d, streams("3.0.0", 1)).unwrap_err();
    assert!(e.contains("pinned key"), "{e}");
    let mut cfg = Config::default();
    cfg.sources = vec![SourceCfg { id: "a".into(), name: "a".into(), kind: SourceKind::Url, location: "https://store.example/catalog.json".into(), enabled: true, pin: pin(1) }];
    std::fs::create_dir_all(d.join("store")).unwrap();
    cfg.save(&d.join("store")).unwrap();
    // a package signed by another key is refused and leaves nothing
    let mut other = Config::default();
    other.sources = vec![SourceCfg { id: "a".into(), name: "a".into(), kind: SourceKind::Url, location: "https://store.example/catalog.json".into(), enabled: true, pin: pin(2) }];
    other.save(&d.join("store")).unwrap();
    assert!(install_with_streamer(&d, streams("3.0.0", 1)).unwrap_err().contains("not signed by the store key you trust"));
    assert!(browserpkg::installed_version(&d).is_none());
    cfg.save(&d.join("store")).unwrap();
    assert_eq!(install_with_streamer(&d, streams("3.0.0", 1)).unwrap(), "3.0.0");
    assert_eq!(browserpkg::installed_version(&d).as_deref(), Some("3.0.0"));
    // the store answering 404 for the program is a plain error, and the installed version stays
    let mut broken = streams("3.1.0", 1);
    Arc::get_mut(&mut broken).unwrap().0.retain(|u, _| u.ends_with("manifest.json"));
    assert!(install_with_streamer(&d, broken).unwrap_err().contains("404"));
    assert_eq!(browserpkg::installed_version(&d).as_deref(), Some("3.0.0"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_cards_lines_come_from_the_pinned_store_with_the_version_and_size_or_the_plain_reason() {
    let d = data("cardlines");
    // no store pinned: the reason, nothing fetched
    let none: Arc<dyn Fetch> = Arc::new(Files(BTreeMap::new()));
    assert!(card_lines_with(&d, none.clone()).unwrap_err().contains("pinned key"));
    let mut cfg = Config::default();
    cfg.sources = vec![SourceCfg { id: "s".into(), name: "s".into(), kind: SourceKind::Url, location: "https://store.example/catalog.json".into(), enabled: true, pin: pin(1) }];
    std::fs::create_dir_all(d.join("store")).unwrap();
    cfg.save(&d.join("store")).unwrap();
    // the store's files for this machine's architecture (the card asks for it)
    let files = |version: &str, k: u8| -> Arc<dyn Fetch> {
        let arch = std::env::consts::ARCH;
        let m = browserpkg::Manifest { version: version.into(), size: 4000, sha256: crate::pkg::hex(&Sha256::digest(vec![3u8; 4000])), arch: arch.into() };
        let mut f = BTreeMap::new();
        f.insert(format!("https://store.example/browser/{arch}/manifest.json"), browserpkg::manifest_doc(&m, &key(k)).to_string().into_bytes());
        Arc::new(Files(f))
    };
    let lines = card_lines_with(&d, files("2026.11-1", 1)).unwrap();
    assert!(lines[0].contains("2026.11-1") && lines[0].contains("1 MB"), "{lines:?}");
    // a store that does not answer, and one signed by another key: plain errors, no panic
    assert!(card_lines_with(&d, none).unwrap_err().contains("404"));
    assert!(card_lines_with(&d, files("9", 2)).unwrap_err().contains("not signed by the store key"));
    let _ = std::fs::remove_dir_all(&d);
}
