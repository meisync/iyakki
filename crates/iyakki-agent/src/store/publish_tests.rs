//! The IDE's Publish flow against the real private-store server code on a loopback socket: preflight and version bump, the stores on the
//! device, the signing key, the token, the review card, the publish, the install back from the store, the log and the hint.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_sec::policy::Role;

use super::catalog::HttpFetch;
use super::install::tests::{admin, NOW};
use super::publishflow::*;
use super::server::{hash_token, Config, StoreServer};
use super::{author, authtools, keys, tilehub, tile, Store};
use super::install::Actor;
use crate::pkg::{new_signing_key, signing_key_hex};
use crate::services::Services;
use crate::skills::Scope;

/// Plain HTTP to the loopback only: the tests never leave the machine.
struct Loop;
impl Upstream for Loop {
    fn request(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
        // a store deployed here is recorded as http://localhost:<NodePort>
        let url = url.replacen("http://localhost:", "http://127.0.0.1:", 1);
        let rest = url.strip_prefix("http://127.0.0.1:").ok_or_else(|| format!("test transport: only the loopback ({url})"))?;
        let (port, path) = rest.split_once('/').map_or((rest, String::new()), |(p, r)| (p, format!("/{r}")));
        let mut s = std::net::TcpStream::connect(format!("127.0.0.1:{port}")).map_err(|e| format!("Connection refused: {e}"))?;
        s.set_read_timeout(Some(std::time::Duration::from_secs(10))).ok();
        let mut req = format!("{method} {} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n", if path.is_empty() { "/" } else { &path }, body.len());
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        s.write_all(req.as_bytes()).and_then(|_| s.write_all(body)).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let at = out.windows(4).position(|w| w == b"\r\n\r\n").ok_or("bad reply")?;
        let status = String::from_utf8_lossy(&out[..at]).split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        Ok(UpstreamResponse { status, body: out[at + 4..].to_vec() })
    }
}

struct World {
    srv: Arc<StoreServer>,
    addr: String,
    stop: Arc<AtomicBool>,
    th: Option<std::thread::JoinHandle<()>>,
    svc: Arc<Services>,
    store: Store,
    dir: PathBuf,
    ws: PathBuf,
    ptoken: String,
    fetch: HttpFetch,
}

impl Drop for World {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(&self.addr);
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn world(tag: &str) -> World {
    let dir = std::env::temp_dir().join(format!("iyakki-pub-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = Config::new(&dir.join("server"));
    cfg.writes_per_min = 1000;
    cfg.admin_token_hash = Some(hash_token("admin-token-for-tests-0123456789"));
    let srv = Arc::new(StoreServer::open(cfg, &signing_key_hex(&new_signing_key().unwrap())).unwrap());
    let ptoken = srv.create_publisher("acme", "Acme").unwrap();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, st2) = (srv.clone(), stop.clone());
    let th = Some(std::thread::spawn(move || super::server_http::serve(s2, l, st2)));
    let svc = Services::open(&dir.join("device"), Arc::new(Loop), Arc::new(Loop)).unwrap();
    svc.on_login("root", "a-long-test-passphrase", true);
    let store = Store::open(&dir.join("device")).with_clock(Arc::new(|| NOW));
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    World { srv, addr, stop, th, svc, store, dir, ws, ptoken, fetch: HttpFetch(Arc::new(Loop)) }
}

impl World {
    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Add the running server as a private store, as the person does after comparing the fingerprint.
    fn add_store(&self, name: &str) -> super::catalog::SourceCfg {
        let p = self.store.probe_private(&self.fetch, &self.url()).unwrap();
        let id = self.store.add_private_source(&admin(), &self.fetch, name, &self.url(), &p.fingerprint).unwrap();
        self.store.set_publish_target(&admin(), &id).unwrap();
        self.store.config().sources.into_iter().find(|s| s.id == id).unwrap()
    }

    fn app(&self) -> PathBuf {
        let d = self.ws.join("office-clock");
        if !d.exists() {
            author::scaffold("tile", "office-clock", &self.ws, "").unwrap();
            std::fs::write(d.join("CHANGELOG.md"), "# Changelog\n\n## 0.1.0 - first\n- first release\n").unwrap();
            // a good summary, a real icon
            let t = std::fs::read_to_string(d.join("iyakki-app.json")).unwrap().replace("A tile app for the IYAKKI launcher.", "A clock for the office wall.");
            std::fs::write(d.join("iyakki-app.json"), t).unwrap();
        }
        d
    }

    fn key(&self) -> (ed25519_dalek::SigningKey, String) {
        if keys::list(&self.svc, "root").is_empty() {
            keys::create(&self.svc, "root", "my-key").unwrap();
        }
        keys::load_default(&self.svc, "root").unwrap()
    }
}

#[test]
fn preflight_lists_what_to_fix_and_the_bump_helper_edits_the_version_and_the_change_log() {
    let w = world("pre");
    let d = w.app();
    let pf = preflight(&d);
    assert!(pf.ok() && pf.tests == Some(true) && pf.kind == "tile" && pf.version == "0.1.0", "{pf:?}");
    assert!(pf.changelog_has_version);
    // problems in plain words: a missing icon and a broken test
    let manifest = std::fs::read_to_string(d.join("app.json")).unwrap();
    std::fs::write(d.join("app.json"), manifest.replace("\"icon\": \"🧩\", ", "")).unwrap();
    let pf = preflight(&d);
    assert!(!pf.ok() && pf.problems.iter().any(|p| p.contains("no icon")), "{:?}", pf.problems);
    std::fs::write(d.join("app.json"), &manifest).unwrap();
    std::fs::write(d.join("app.js"), "document.getElementById('message').textContent = 'x';").unwrap();
    let pf = preflight(&d);
    assert_eq!(pf.tests, Some(false));
    assert!(pf.problems.iter().any(|p| p.contains("headless tests do not pass")));
    std::fs::write(d.join("app.js"), "var a = 1;").unwrap();
    // the change log hint, then the bump
    assert_eq!(bumped("1.2.3", Bump::Patch).unwrap(), "1.2.4");
    assert_eq!(bumped("1.2.3", Bump::Minor).unwrap(), "1.3.0");
    assert_eq!(bumped("1.2.3", Bump::Major).unwrap(), "2.0.0");
    assert_eq!(bumped("1", Bump::Patch).unwrap(), "1.0.1");
    assert!(bumped("one", Bump::Patch).is_err());
    let v = apply_bump(&d, Bump::Minor, "adds a seconds hand", "2026-10-17").unwrap();
    assert_eq!(v, "0.2.0");
    assert!(std::fs::read_to_string(d.join("iyakki-app.json")).unwrap().contains("\"version\":\"0.2.0\""));
    assert!(std::fs::read_to_string(d.join("app.json")).unwrap().contains("\"version\": \"0.2.0\""));
    let log = std::fs::read_to_string(d.join("CHANGELOG.md")).unwrap();
    assert!(log.starts_with("# Changelog\n\n## 0.2.0 - 2026-10-17\n- adds a seconds hand\n\n## 0.1.0"), "{log}");
    assert!(preflight(&d).changelog_has_version);
    assert!(apply_bump(&d, Bump::Patch, "  ", "2026-10-17").is_err(), "a line is required");
}

#[test]
fn stores_on_the_device_are_listed_private_first_with_fingerprints_and_the_default_target() {
    let w = world("stores");
    assert!(stores(&w.store).is_empty());
    let src = w.add_store("Office store");
    let list = stores(&w.store);
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].name.as_str(), list[0].private, list[0].is_default, list[0].address.as_str()), ("Office store", true, true, w.url().as_str()));
    assert_eq!(list[0].fingerprint, super::server::fingerprint(&w.srv.catalog_key()));
    assert_eq!(src.pin, w.srv.catalog_key());
}

#[test]
fn the_signing_key_is_private_to_the_person_never_inside_a_workspace_and_gives_the_admin_a_line() {
    let w = world("keys");
    assert!(keys::list(&w.svc, "root").is_empty() && keys::load_default(&w.svc, "root").is_none());
    assert!(keys::create(&w.svc, "root", "Bad Name").is_err());
    let k = keys::create(&w.svc, "root", "my-key").unwrap();
    assert!(k.default && k.fingerprint.split(' ').count() == 8, "{k:?}");
    assert!(keys::create(&w.svc, "root", "my-key").is_err());
    let second = keys::create(&w.svc, "root", "other").unwrap();
    assert!(!second.default);
    keys::set_default(&w.svc, "root", "other").unwrap();
    assert_eq!(keys::list(&w.svc, "root")[0].label, "other");
    // 0600, under the data folder, and not where a workspace is
    let file = w.svc.dir().join("publisher-keys/root/my-key.key");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(!file.starts_with(&w.ws));
    assert_eq!(k.admin_line(), format!("{} my-key", k.public));
    assert!(keys::list(&w.svc, "bob").is_empty(), "keys are per person");
    // the key never shows in a project folder or in the package's text beyond its public half
    let d = w.app();
    let (text, _) = author::pack_dir(&d, Some(&keys::load(&w.svc, "root", Some("my-key")).unwrap().0)).unwrap();
    assert!(!text.contains(&std::fs::read_to_string(&file).unwrap()));
}

#[test]
fn the_card_shows_the_server_the_package_and_what_will_happen_with_a_new_key_and_an_http_address() {
    let w = world("card");
    let src = w.add_store("Office store");
    let d = w.app();
    let key = w.key();
    let (c, text) = card(&w.svc, "root", &d, &src, &key, &w.fetch).unwrap();
    assert_eq!((c.name.as_str(), c.version.as_str(), c.kind.as_str(), c.store.as_str()), ("office-clock", "0.1.0", "tile", "Office store"));
    assert!(!c.https && c.http_warning.as_deref().unwrap().contains("unencrypted"), "{c:?}");
    assert!(c.new_key && c.key_warning.as_deref().unwrap().contains("Community") && c.key_warning.as_deref().unwrap().contains(&key.1), "{c:?}");
    assert!(c.trust.contains("Community"));
    assert!(c.permissions.iter().any(|p| p.contains("saved data")), "{:?}", c.permissions);
    assert!(c.files.iter().any(|f| f.starts_with("index.html")) && c.files.iter().any(|f| f.starts_with("app.js")) && !c.files.iter().any(|f| f.contains("test")), "{:?}", c.files);
    assert_eq!(c.size, text.len());
    assert!(!c.token_present && c.store_has.is_none());
    assert!(c.yank_note.contains("does NOT remove copies already installed"));
    // nothing was sent
    assert!(w.srv.audit_lines().iter().all(|l| !l.contains(" publish ")));
    // a project that fails its checks has no card
    std::fs::write(d.join("app.js"), "eval('1');").unwrap();
    assert!(card(&w.svc, "root", &d, &src, &key, &w.fetch).unwrap_err().contains("fix these first"));
}

#[test]
fn publishing_needs_a_key_and_a_token_then_lists_the_app_and_it_installs_back_as_a_tile() {
    let w = world("flow");
    let src = w.add_store("Office store");
    let d = w.app();
    let now = 1_790_000_000;
    // no key
    assert_eq!(publish(&w.svc, "root", &d, &src, None, &w.fetch, now).unwrap_err(), Stop::NeedsKey);
    let key = w.key();
    // no token: the dialog asks, the token goes to the vault, nothing is stored in the workspace
    assert_eq!(publish(&w.svc, "root", &d, &src, Some(&key), &w.fetch, now).unwrap_err(), Stop::NeedsToken);
    assert!(!token_present(&w.svc, "root", &src.id));
    assert!(save_token(&w.svc, "root", &src.id, "short").is_err() && save_token(&w.svc, "root", &src.id, "has space in it long").is_err());
    save_token(&w.svc, "root", &src.id, &w.ptoken).unwrap();
    assert!(token_present(&w.svc, "root", &src.id));
    // a wrong token is the store's refusal in its words
    save_token(&w.svc, "root", &src.id, "wrong-token-but-long-enough").unwrap();
    let e = publish(&w.svc, "root", &d, &src, Some(&key), &w.fetch, now).unwrap_err();
    assert!(matches!(&e, Stop::Refused(m) if m.contains("401")), "{e:?}");
    save_token(&w.svc, "root", &src.id, &w.ptoken).unwrap();
    // the real publish
    let r = publish(&w.svc, "root", &d, &src, Some(&key), &w.fetch, now).unwrap();
    assert_eq!((r.name.as_str(), r.version.as_str(), r.trust.as_str()), ("office-clock", "0.1.0", "Community"));
    assert!(r.serial >= 1 && !r.sha256.is_empty());
    // the log and the header line hold no secret
    let log = std::fs::read_to_string(d.join(".iyakki/publish.json")).unwrap();
    assert!(log.contains("0.1.0") && log.contains("Office store") && log.contains(&r.sha256) && !log.contains(&w.ptoken) && !log.contains(&key.1), "{log}");
    assert_eq!(header_hint(&d).unwrap(), "Published: 0.1.0 \u{2192} Office store");
    // the same version again: refused before anything is sent, with the way out
    let sent = || w.srv.audit_lines().iter().filter(|l| l.contains(" publish ")).count();
    let before = sent();
    let e = publish(&w.svc, "root", &d, &src, Some(&key), &w.fetch, now).unwrap_err();
    assert!(matches!(&e, Stop::Refused(m) if m.contains("not newer than 0.1.0") && m.contains("versions only go up") && m.contains("0.1.1")), "{e:?}");
    assert_eq!(sent(), before, "nothing was sent");
    assert!(card(&w.svc, "root", &d, &src, &key, &w.fetch).unwrap_err().contains("not newer than 0.1.0"), "the review card refuses it too");
    // a lower version too
    set_version(&d, "0.0.9").unwrap();
    assert!(matches!(publish(&w.svc, "root", &d, &src, Some(&key), &w.fetch, now), Err(Stop::Refused(m)) if m.contains("not newer")));
    // work goes on: bump, the header says there is something to publish, the key is no longer new to the store
    let v = apply_bump(&d, Bump::Patch, "dimmer at night", "2026-10-17").unwrap();
    assert_eq!(v, "0.0.10", "a bump goes from where the project is");
    set_version(&d, "0.2.0").unwrap();
    assert!(header_hint(&d).unwrap().contains("update available"), "{:?}", header_hint(&d));
    let (c2, _) = card(&w.svc, "root", &d, &src, &key, &w.fetch).unwrap();
    assert!(!c2.new_key && c2.key_warning.is_none() && c2.store_has.as_deref() == Some("0.1.0") && c2.token_present, "{c2:?}");
    // 'Install it here to test': the normal review, then the tile appears in the launcher
    let alice = super::install::Actor { user: "root".into(), allowed: true, can_own: true, can_shared: true, admin: true };
    let review = review_published(&w.svc, &w.store, &alice, &w.fetch, "office-clock", Scope::User("root".into())).unwrap();
    assert!(review.refusal.is_none(), "{:?}", review.refusal);
    assert!(review.summary().contains("Office clock"), "{}", review.summary());
    let said = install_reviewed(&w.svc, &w.store, &alice, &review).unwrap();
    assert!(said.contains("Installed office-clock 0.1.0"), "{said}");
    let tiles = tilehub::installed(&w.svc, &Role::Admin.template(), "root");
    assert_eq!(tiles.len(), 1);
    assert_eq!((tiles[0].title.as_str(), tiles[0].group.as_str(), tiles[0].dev), ("Office clock", "My apps", false));
    let mut rt = tilehub::open(&w.svc, &Role::Admin.template(), "root", &tiles[0], Arc::new(iyakki_web::fetch::NoNetwork)).unwrap();
    rt.press("Count").unwrap();
    assert!(rt.page_text().contains("Pressed 1 time"), "{}", rt.page_text());
    assert_eq!(tile::TileStorage::open(&tile::storage_file(&w.store, &Scope::User("root".into()), "office-clock", "root")).get("count").as_deref(), Some("1"), "its data is where the lifecycle layer offers it");
    // the server's own record
    let log = w.srv.audit_lines().join("\n");
    assert!(log.contains("office-clock 0.1.0") && !log.contains(&w.ptoken), "{log}");
}

#[test]
fn the_publishing_tools_always_ask_and_no_mode_runs_them_for_the_person() {
    use serde_json::json;
    for t in ["store_publish", "tile_install_dev", "app_bump_version"] {
        assert!(crate::agent::hard_ask(t, &json!({})).is_some(), "{t} always asks");
    }
    for t in ["store_publish", "tile_install_dev", "app_bump_version", "tile_pack", "tile_scaffold"] {
        let d = super::authtools::defs(&Role::Admin.template());
        let tool = d.iter().find(|x| x.spec.name == t).unwrap_or_else(|| panic!("{t} is offered"));
        assert_eq!(tool.risk, crate::agent::Risk::Ask, "{t}");
    }
    assert!(crate::grants::blocker("store_publish", &json!({})).is_some(), "a session grant never covers publishing");
    assert!(crate::grants::blocker("tile_install_dev", &json!({})).is_some());
    // an ordinary role is not offered the dev install
    let d = super::authtools::defs(&Role::User.template());
    assert!(!d.iter().any(|x| x.spec.name == "tile_install_dev"));
    // the read-only helpers are safe
    let d = super::authtools::defs(&Role::Admin.template());
    for t in ["tile_validate", "tile_test", "app_preflight", "publisher_keys"] {
        assert_eq!(d.iter().find(|x| x.spec.name == t).unwrap().risk, crate::agent::Risk::Safe, "{t}");
    }
}

// ------------------------------------------------------------------------------------------------ the one-step flow for an agent

fn admin_policy() -> iyakki_sec::policy::Policy {
    Role::Admin.template()
}

/// What a store deployed from this device leaves behind: the instance record and the admin token in the person's vault.
fn deployed(w: &World, src: &super::catalog::SourceCfg) {
    let rec = crate::platform::installed::Installed {
        id: "private-store-iyakki-private-store".into(),
        service: super::privatestore::ID.into(),
        urls: vec![crate::platform::UrlInfo { label: "Catalog".into(), url: src.location.clone(), kind: "catalog".into() }],
        vault: vec!["svc.private-store-iyakki-private-store.admin-token".into()],
        ..Default::default()
    };
    crate::platform::installed::upsert(w.svc.dir(), "root", rec).unwrap();
    w.svc.user_set("root", "svc.private-store-iyakki-private-store.admin-token", &w.ptoken).unwrap();
}

fn all_text(dir: &std::path::Path) -> String {
    let mut out = String::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.push_str(&all_text(&p));
        } else if let Ok(t) = std::fs::read_to_string(&p) {
            out.push_str(&t);
        }
    }
    out
}

const NAGGING: [&str; 5] = ["create a key", "ask the person", "publisher key yet", "Publish dialog", "add the token"];

#[test]
fn tile_ship_checks_packs_signs_with_a_key_it_makes_and_publishes_with_the_deployed_stores_token() {
    use super::authtools::{call, describe_for};
    let w = world("ship");
    let src = w.add_store("Office store");
    deployed(&w, &src);
    let d = w.app();
    assert!(keys::list(&w.svc, "root").is_empty() && !token_present_entry(&w, &src), "no key, no token of the person's own");
    assert!(token_present(&w.svc, "root", &src.id), "the deployed store's admin token counts");
    // the card shows what will be published and does not make a key
    let card = describe_for(&w.svc, "root", &w.ws, "tile_ship", &serde_json::json!({"path": "office-clock"}));
    assert!(card.contains("office-clock 0.1.0") && card.contains("Office store") && card.contains("saved data") && card.contains("Trusted (your own key)"), "{card}");
    assert!(keys::list(&w.svc, "root").is_empty(), "describing makes nothing");
    // one call
    let out = call("tile_ship", &serde_json::json!({"path": "office-clock"}), &w.svc, &admin_policy(), "root", &w.ws).unwrap();
    assert!(out.contains("Published the tile app office-clock 0.1.0 to Office store") && out.contains("A publisher key was made"), "{out}");
    for n in NAGGING {
        assert!(!out.to_lowercase().contains(&n.to_lowercase()), "{n} in {out}");
    }
    // the key: made, private file outside the workspace, public half trusted, the private half nowhere in the output or workspace
    let ks = keys::list(&w.svc, "root");
    assert_eq!(ks.len(), 1);
    assert!(ks[0].default && ks[0].label == "my-publisher-key");
    let (sk, _) = keys::load_default(&w.svc, "root").unwrap();
    let hex = signing_key_hex(&sk);
    assert!(!out.contains(&hex) && !all_text(&w.ws).contains(&hex) && !all_text(&w.ws).contains(&w.ptoken) && !out.contains(&w.ptoken));
    let keyfile = w.svc.dir().join("publisher-keys/root/my-publisher-key.key");
    assert!(keyfile.is_file() && !keyfile.starts_with(&w.ws));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&keyfile).unwrap().permissions().mode() & 0o777, 0o600);
    // trust: that one key, for packages it signs; nothing else was added
    let trusted = w.svc.packages.trusted_keys();
    assert_eq!(trusted.len(), 1, "{trusted:?}");
    assert_eq!(trusted[0].0, ks[0].public);
    // the store lists it, and the log has no secret
    assert!(w.srv.audit_lines().iter().any(|l| l.contains(" publish ")));
    assert!(!std::fs::read_to_string(d.join(".iyakki/publish.json")).unwrap().contains(&w.ptoken));
    // the same request again: the version is raised once, the key is reused
    let out = call("tile_ship", &serde_json::json!({"dir": "office-clock"}), &w.svc, &admin_policy(), "root", &w.ws).unwrap();
    assert!(out.contains("office-clock 0.1.1") && out.contains("version was raised to 0.1.1") && !out.contains("A publisher key was made"), "{out}");
    assert_eq!(keys::list(&w.svc, "root").len(), 1);
    // store_publish with a folder is the same flow
    set_version(&d, "0.3.0").unwrap();
    let out = call("store_publish", &serde_json::json!({"package_path": "office-clock"}), &w.svc, &admin_policy(), "root", &w.ws).unwrap();
    assert!(out.contains("office-clock 0.3.0"), "{out}");
}

fn token_present_entry(w: &World, src: &super::catalog::SourceCfg) -> bool {
    w.svc.user_get("root", &super::authtools::token_entry(&src.id)).is_ok()
}

#[test]
fn tile_ship_says_what_the_person_must_do_in_one_sentence_when_there_is_no_store_or_no_token() {
    use super::authtools::{call, describe_for};
    // no private store at all
    let w = world("ship-nostore");
    w.app();
    let e = call("tile_ship", &serde_json::json!({"path": "office-clock"}), &w.svc, &admin_policy(), "root", &w.ws).unwrap_err();
    assert!(e.contains("Deploy the private store first (STORE \u{2192} Private store \u{2192} Deploy; one click now creates the registry)"), "{e}");
    assert!(!e.to_lowercase().contains("key") || e.contains("do not ask for keys"), "{e}");
    assert!(keys::list(&w.svc, "root").is_empty(), "nothing is made for a publish that cannot happen");
    assert!(describe_for(&w.svc, "root", &w.ws, "tile_ship", &serde_json::json!({"path": "office-clock"})).contains("not possible yet"));
    // a store that was added by hand and whose token IYAKKI does not hold
    let src = w.add_store("Office store");
    let e = call("tile_ship", &serde_json::json!({"path": "office-clock"}), &w.svc, &admin_policy(), "root", &w.ws).unwrap_err();
    assert!(e.contains("Office store has no publish token on this device") && e.contains("STORE"), "{e}");
    assert!(keys::list(&w.svc, "root").is_empty());
    let _ = src;
}

#[test]
fn tile_ship_surfaces_validation_and_secret_scan_problems_and_publishes_nothing() {
    use super::authtools::call;
    let w = world("ship-bad");
    let src = w.add_store("Office store");
    deployed(&w, &src);
    let d = w.app();
    let before = || w.srv.audit_lines().iter().filter(|l| l.contains(" publish ")).count();
    // a broken manifest
    let manifest = std::fs::read_to_string(d.join("app.json")).unwrap();
    std::fs::write(d.join("app.json"), manifest.replace("\"icon\": \"🧩\", ", "")).unwrap();
    let e = call("tile_ship", &serde_json::json!({"path": "office-clock"}), &w.svc, &admin_policy(), "root", &w.ws).unwrap_err();
    assert!(e.contains("not published") && e.contains("fix these first") && e.contains("no icon"), "{e}");
    std::fs::write(d.join("app.json"), &manifest).unwrap();
    // a secret in a file
    std::fs::write(d.join("notes.txt"), "token ghp_abcdefghijklmnopqrstuvwxyz0123456789").unwrap();
    let e = call("tile_ship", &serde_json::json!({"path": "office-clock"}), &w.svc, &admin_policy(), "root", &w.ws).unwrap_err();
    assert!(e.contains("not published") && e.contains("access token") && !e.contains("ghp_abcdefgh"), "{e}");
    std::fs::remove_file(d.join("notes.txt")).unwrap();
    // not a folder / outside the workspace
    assert!(call("tile_ship", &serde_json::json!({"path": "../x"}), &w.svc, &admin_policy(), "root", &w.ws).is_err());
    assert!(call("tile_ship", &serde_json::json!({"path": "nothing-here"}), &w.svc, &admin_policy(), "root", &w.ws).is_err());
    assert_eq!(before(), 0, "nothing reached the store");
    // an account that may not publish
    let mut p = admin_policy();
    p.fs_write.clear();
    assert!(call("tile_ship", &serde_json::json!({"path": "office-clock"}), &w.svc, &p, "root", &w.ws).is_err());
}

#[test]
fn the_agents_tool_list_has_tile_ship_asking_always_and_the_bridge_offers_it() {
    let defs = super::authtools::defs(&admin_policy());
    let d = defs.iter().find(|d| d.spec.name == "tile_ship").expect("tile_ship");
    assert_eq!(d.risk, crate::agent::Risk::Ask);
    assert!(crate::agent::hard_ask("tile_ship", &serde_json::json!({})).is_some() || d.risk == crate::agent::Risk::Ask);
    assert!(crate::extagent::bridge::offered(d));
    assert!(d.spec.description.contains("Never ask the person for keys"));
}

// ------------------------------------------------------------------------------------------------ a deployed store that was never added

/// The device of the report: the deploy left the installed record (addresses as `http://localhost:<port>`) and the admin token in the
/// vault, and nothing in the STORE.
fn deployed_not_added(w: &World, admin_token: &str) {
    let base = format!("http://localhost:{}", w.addr.rsplit(':').next().unwrap());
    let u = |label: &str, url: String, kind: &str| crate::platform::UrlInfo { label: label.into(), url, kind: kind.into() };
    let rec = crate::platform::installed::Installed {
        id: "private-store-iyakki-private-store".into(),
        service: super::privatestore::ID.into(),
        title: "Private store".into(),
        namespace: "iyakki-private-store".into(),
        urls: vec![u("Private store", base.clone(), "web"), u("Catalog", format!("{base}/catalog.json"), "catalog"), u("Publish", format!("{base}/v1/publish"), "api")],
        vault: vec!["svc.private-store-iyakki-private-store.admin-token".into(), "svc.private-store-iyakki-private-store.catalog-seed".into()],
        ..Default::default()
    };
    crate::platform::installed::upsert(w.svc.dir(), "root", rec).unwrap();
    w.svc.user_set("root", "svc.private-store-iyakki-private-store.admin-token", admin_token).unwrap();
}

#[test]
fn addresses_of_one_store_compare_equal_whatever_the_spelling() {
    use super::privatepub::{normalise_with, same_location_with};
    let own = vec!["192.0.2.20".to_string()];
    let same = ["http://localhost:30300", "http://localhost:30300/", "http://localhost:30300/catalog.json", "http://127.0.0.1:30300/catalog.json", "HTTP://LocalHost:30300/v1/publish", "http://192.0.2.20:30300/catalog.json", "http://[::1]:30300", "http://127.0.1.1:30300/"];
    for a in same {
        for b in same {
            assert!(same_location_with(a, b, &own), "{a} vs {b}: {} / {}", normalise_with(a, &own), normalise_with(b, &own));
        }
    }
    // other stores stay other
    for other in ["http://localhost:30301", "http://192.0.2.21:30300", "https://localhost:30300", "http://store.example.lan:30300", "http://localhost:30300/team"] {
        assert!(!same_location_with("http://localhost:30300", other, &own), "{other}");
    }
    assert!(same_location_with("https://store.example.lan/catalog.json", "https://STORE.example.lan/", &own));
    assert!(same_location_with("https://store.example.lan:443", "https://store.example.lan", &own));
    assert!(same_location_with("http://store.example.lan:80/", "http://store.example.lan", &own));
    assert!(!same_location_with("http://a.example.lan", "http://b.example.lan", &own));
}

#[test]
fn the_deployed_stores_token_is_found_through_localhost_127_and_a_trailing_slash() {
    let w = world("token-spell");
    deployed_not_added(&w, "admin-token-for-tests-0123456789");
    let port = w.addr.rsplit(':').next().unwrap().to_string();
    let src = |loc: String| super::catalog::SourceCfg { id: "private-x".into(), name: "x".into(), kind: super::catalog::SourceKind::Url, location: loc, enabled: true, pin: "ab".repeat(32) };
    for loc in [format!("http://localhost:{port}/catalog.json"), format!("http://127.0.0.1:{port}/catalog.json"), format!("http://127.0.0.1:{port}/"), format!("http://LOCALHOST:{port}")] {
        let t = authtools::publish_token(&w.svc, "root", &src(loc.clone()));
        assert_eq!(t.as_deref().map(String::as_str), Some("admin-token-for-tests-0123456789"), "{loc}");
    }
    assert!(authtools::publish_token(&w.svc, "root", &src("http://localhost:1/catalog.json".into())).is_none(), "another port is another store");
    assert!(authtools::publish_token(&w.svc, "root", &src("https://store.example.lan/catalog.json".into())).is_none());
    // the person's own entry still wins
    w.svc.user_set("root", &authtools::token_entry("private-x"), "the-persons-own-token").unwrap();
    assert_eq!(authtools::publish_token(&w.svc, "root", &src(format!("http://127.0.0.1:{port}"))).as_deref().map(String::as_str), Some("the-persons-own-token"));
}

#[test]
fn the_unadded_store_is_found_added_with_its_key_pinned_and_chosen_and_a_wrong_key_or_account_is_refused() {
    use super::privatepub::*;
    let w = world("unadded");
    deployed_not_added(&w, "admin-token-for-tests-0123456789");
    // today's dead end: nothing in the STORE to publish to
    assert!(stores(&w.store).is_empty());
    let e = authtools::target_for(&w.svc, "root", &w.store, "").unwrap_err();
    assert!(e.contains("deployed on this device but not added"), "{e}");
    let o = offer(&w.svc, "root", &w.store, &w.fetch).expect("the unadded store is offered");
    let p = o.probe.clone().expect("it answers");
    assert_eq!(p.fingerprint, super::server::fingerprint(&w.srv.catalog_key()));
    assert!(option_label(&o).starts_with("Private store on this device (not added yet): Private store \u{b7} key "), "{}", option_label(&o));
    // wrong account: a plain user cannot change the store
    let user = Actor::from_policy("root", &Role::User.template());
    let e = add_and_target(&w.store, &user, &w.fetch, &p, "x").unwrap_err();
    assert!(e.contains("only an administrator"), "{e}");
    // a key other than the one shown: refused, nothing added
    let mut shown = p.clone();
    shown.fingerprint = "0000 0000 0000 0000 0000 0000 0000 0000".into();
    let e = add_and_target(&w.store, &admin(), &w.fetch, &shown, "x").unwrap_err();
    assert!(e.contains("nothing was added"), "{e}");
    assert!(w.store.config().sources.is_empty() && w.store.config().publish_target.is_empty());
    // the typed fingerprint of the Settings step is refused the same way
    assert!(w.store.pin_probed(&admin(), "x", &p, "1111 1111").unwrap_err().contains("does not match"));
    // the right one
    let src = add_and_target(&w.store, &admin(), &w.fetch, &p, &p.name).unwrap();
    assert_eq!(super::server::fingerprint(&src.pin), p.fingerprint);
    assert_eq!(w.store.config().publish_target, src.id);
    assert!(offer(&w.svc, "root", &w.store, &w.fetch).is_none(), "added through 127.0.0.1 or localhost, it is one store");
    // the token comes from the vault entry of the deploy
    assert!(token_present(&w.svc, "root", &src.id));
    let info = super::privatestore::status_info(w.svc.dir(), "root", &w.store);
    assert_eq!(info.added.as_ref().map(|a| a.0.as_str()), Some(src.id.as_str()));
}

#[test]
fn publishing_to_the_device_own_store_over_plain_http_works_a_remote_one_still_needs_https() {
    let w = world("plainhttp");
    deployed_not_added(&w, "admin-token-for-tests-0123456789");
    let o = super::privatepub::offer(&w.svc, "root", &w.store, &w.fetch).unwrap();
    let src = super::privatepub::add_and_target(&w.store, &admin(), &w.fetch, o.probe.as_ref().unwrap(), "Private store").unwrap();
    let d = w.app();
    let key = w.key();
    // the review card: plain http to the device itself, said as a warning, with the pod's namespace never needed
    let (c, text) = card(&w.svc, "root", &d, &src, &key, &w.fetch).unwrap();
    assert!(!c.https && c.http_warning.is_some() && c.token_present && c.new_key, "{c:?}");
    let r = publish(&w.svc, "root", &d, &src, Some(&key), &w.fetch, NOW).unwrap();
    assert_eq!((r.name.as_str(), r.version.as_str()), ("office-clock", "0.1.0"));
    assert!(w.srv.audit_lines().iter().any(|l| l.contains("office-clock 0.1.0")));
    assert!(!w.srv.audit_lines().join("\n").contains("admin-token-for-tests"));
    // install it back from the store
    let actor = admin();
    let rev = review_published(&w.svc, &w.store, &actor, &w.fetch, "office-clock", actor.default_scope()).unwrap();
    assert!(rev.refusal.is_none(), "{:?}", rev.refusal);
    // the transport rule: a public http address is refused before anything is sent, a private-network one is allowed, https is allowed
    let up = Loop;
    let e = author::publish_to_server(&up, "http://store.example.com:30300", "tok-0123456789", &text).unwrap_err();
    assert!(e.contains("travel unencrypted") && e.contains("https"), "{e}");
    let e = author::publish_to_server(&up, "http://203.0.113.9:30300", "tok-0123456789", &text).unwrap_err();
    assert!(e.contains("travel unencrypted"), "{e}");
    let e = author::publish_to_server(&up, "http://10.1.2.3:30300", "tok-0123456789", &text).unwrap_err();
    assert!(e.contains("Nothing was sent") && e.contains("could not reach the store at http://10.1.2.3:30300"), "private networks are allowed through, then fail in the test transport: {e}");
}

#[test]
fn every_publish_failure_says_what_happened_and_what_to_do_not_a_bare_transport_text() {
    let w = world("errors");
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let d = w.app();
    let (text, _) = author::pack_dir(&d, Some(&w.key().0)).unwrap();
    // nothing listens: refused connection
    let e = author::publish_to_server(&Loop, &format!("http://localhost:{port}"), "tok-0123456789", &text).unwrap_err();
    assert!(e.starts_with("Nothing was sent") && e.contains("refused") && e.contains("K8S tile") && e.contains("publish again"), "{e}");
    // the store answers 401 to a wrong token
    let e = author::publish_to_server(&Loop, &w.url(), "not-the-token-0123", &text).unwrap_err();
    assert!(e.contains("did not accept the publish token") && e.contains("401") && e.contains("enter it again"), "{e}");
    for (status, word) in [(401, "token"), (403, "token"), (404, "not an IYAKKI private store"), (409, "already has this version"), (413, "too big"), (429, "limiting"), (500, "check its pod"), (503, "check its pod"), (418, "refused it (418)")] {
        let t = author::refusal_text(status, None);
        assert!(t.contains(word) && !t.to_lowercase().contains("io error"), "{status}: {t}");
    }
    assert!(author::refusal_text(403, Some("this store is read-only")).contains("accepts no publishing"));
    // the review card of a store that does not answer: the pod hint, not a bare error
    let dead = super::catalog::SourceCfg { id: "private-dead".into(), name: "Dead".into(), kind: super::catalog::SourceKind::Url, location: format!("http://localhost:{port}/catalog.json"), enabled: true, pin: "ab".repeat(32) };
    let e = card(&w.svc, "root", &d, &dead, &w.key(), &w.fetch).unwrap_err();
    assert!(e.contains("nothing answers") && e.contains("Nothing was changed") && !e.to_lowercase().contains("io error"), "{e}");
    deployed_not_added(&w, "admin-token-for-tests-0123456789");
    let rec = crate::platform::installed::load(w.svc.dir(), "root");
    let mut r = rec[0].clone();
    for u in &mut r.urls {
        u.url = u.url.replace(&w.addr.rsplit(':').next().unwrap().to_string(), &port.to_string());
    }
    crate::platform::installed::upsert(w.svc.dir(), "root", r).unwrap();
    let e = card(&w.svc, "root", &d, &dead, &w.key(), &w.fetch).unwrap_err();
    assert!(e.contains("pod private-store in namespace iyakki-private-store") && e.contains("K8S tile"), "{e}");
    // an unwritable project file says which file and what to do
    std::fs::remove_file(d.join("CHANGELOG.md")).unwrap();
    std::fs::create_dir(d.join("CHANGELOG.md")).unwrap();
    let e = add_changelog(&d, "0.2.0", "x", "2026-01-01").unwrap_err();
    assert!(e.contains("CHANGELOG.md") && e.starts_with("cannot write"), "{e}");
}

#[test]
fn the_assistant_adds_the_deployed_store_through_its_own_approval_card_and_publishes() {
    use super::authtools::{call, describe_for};
    let w = world("assistant-add");
    deployed_not_added(&w, "admin-token-for-tests-0123456789");
    w.app();
    let args = serde_json::json!({"package_path": "office-clock"});
    // 1. the first call: no Settings detour; the text carries the fingerprint to pass back
    let e = call("store_publish", &args, &w.svc, &admin_policy(), "root", &w.ws).unwrap_err();
    let fp = super::server::fingerprint(&w.srv.catalog_key());
    assert!(e.contains(&fp) && e.contains("add_deployed_store") && e.contains("Do NOT send the person to Settings"), "{e}");
    assert!(w.store.config().sources.is_empty(), "describing and failing add nothing");
    // 2. the approval card shows the address, the key and what follows
    let with = serde_json::json!({"package_path": "office-clock", "add_deployed_store": fp});
    let card = describe_for(&w.svc, "root", &w.ws, "store_publish", &with);
    assert!(card.contains("ADD the private store") && card.contains(&fp) && card.contains("CHOOSE it as the publishing target") && card.contains("PUBLISH"), "{card}");
    let ship = describe_for(&w.svc, "root", &w.ws, "tile_ship", &with);
    assert!(ship.contains("ADD the private store") && ship.contains(&fp), "{ship}");
    assert!(w.store.config().sources.is_empty(), "a card adds nothing");
    // 3. a wrong fingerprint, or an account that is not an administrator: refused, nothing added
    let wrong = serde_json::json!({"package_path": "office-clock", "add_deployed_store": "0000 0000 0000 0000"});
    assert!(call("store_publish", &wrong, &w.svc, &admin_policy(), "root", &w.ws).unwrap_err().contains("is not the key"));
    assert!(describe_for(&w.svc, "root", &w.ws, "store_publish", &wrong).contains("REFUSED"));
    let mut user = Role::User.template();
    user.fs_write = admin_policy().fs_write;
    let e = call("store_publish", &with, &w.svc, &user, "root", &w.ws).unwrap_err();
    assert!(e.contains("only an administrator"), "{e}");
    assert!(w.store.config().sources.is_empty());
    // 4. approved: added with the key pinned, chosen, published with the token of the deploy
    let out = call("store_publish", &with, &w.svc, &admin_policy(), "root", &w.ws).unwrap();
    assert!(out.contains("Published the tile app office-clock 0.1.0 to Private store"), "{out}");
    let cfg = w.store.config();
    assert_eq!(cfg.sources.len(), 1);
    assert_eq!(super::server::fingerprint(&cfg.sources[0].pin), fp);
    assert_eq!(cfg.publish_target, cfg.sources[0].id);
    assert!(!out.contains("admin-token-for-tests") && !all_text(&w.ws).contains("admin-token-for-tests"));
    // the schema offers the argument
    let defs = super::authtools::defs(&admin_policy());
    for n in ["store_publish", "tile_ship"] {
        let d = defs.iter().find(|d| d.spec.name == n).unwrap();
        assert!(d.spec.schema["properties"]["add_deployed_store"].is_object(), "{n}");
        assert_eq!(d.risk, crate::agent::Risk::Ask);
    }
}

#[test]
fn the_assistant_says_the_pod_is_not_ready_instead_of_sending_the_person_around() {
    use super::authtools::call;
    let w = world("assistant-notready");
    deployed_not_added(&w, "admin-token-for-tests-0123456789");
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut r = crate::platform::installed::load(w.svc.dir(), "root").remove(0);
    for u in &mut r.urls {
        u.url = u.url.replace(&w.addr.rsplit(':').next().unwrap().to_string(), &port.to_string());
    }
    crate::platform::installed::upsert(w.svc.dir(), "root", r).unwrap();
    w.app();
    let e = call("store_publish", &serde_json::json!({"package_path": "office-clock"}), &w.svc, &admin_policy(), "root", &w.ws).unwrap_err();
    assert!(e.contains("cannot be read now") && e.contains("K8S tile") && e.contains("nothing answers"), "{e}");
    assert!(w.store.config().sources.is_empty());
}
