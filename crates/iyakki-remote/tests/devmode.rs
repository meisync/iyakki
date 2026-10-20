//! Developer mode end to end over loopback TLS: the real listener, the real client of `iyakki dev`, the real STORE pipeline in a
//! temporary data folder. Pairing (good and bad codes), a signature on every endpoint, a revoked key, roles, install / upgrade /
//! uninstall, logs, open, screenshot, the live-reload watch, the audit trail, and everything refused while developer mode is off.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_agent::devmode::client::{self, DevClient, Device, Home};
use iyakki_agent::devmode::{self, cli, DevMode};
use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_agent::HostApi;
use iyakki_remote::devserver::{DevServer, SCREEN_USER};
use iyakki_sec::policy::{Policy, Role};
use iyakki_shell::accounts::{AuditLog, UserStore};
use iyakki_shell::shell::ShellEnv;

struct Offline;
impl Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct Host;
impl HostApi for Host {
    fn detect_env(&self) -> String {
        "test".into()
    }
    fn net_status(&self) -> String {
        "none".into()
    }
    fn run_wasm(&self, _: &str, _: &Policy, _: &[u8], _: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn power_off(&self) {}
    fn reboot(&self) {}
}

struct Fx {
    env: ShellEnv,
    dir: PathBuf,
    server: DevServer,
    dm: DevMode,
}

impl Drop for Fx {
    fn drop(&mut self) {
        self.server.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    let d = std::env::temp_dir().join(format!("iyakki-devmode-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let mut store = UserStore::open(&d.join("users.json")).unwrap();
    store.create_first_admin("root", "rootpass12").unwrap();
    store.add_local("bob", "bobpass1234", Role::User).unwrap();
    let up: Arc<dyn Upstream> = Arc::new(Offline);
    let env = ShellEnv {
        store: Arc::new(Mutex::new(store)),
        audit: Arc::new(AuditLog::new(&d.join("audit.log"))),
        host: Arc::new(Host),
        cfg: iyakki_agent::config::AiConfig::parse("").unwrap(),
        chat: Arc::new(iyakki_agent::chat::Unconfigured),
        mcp_servers: vec![],
        services: iyakki_agent::services::Services::open(&d.join("svc"), up.clone(), up).unwrap(),
        web: Arc::new(iyakki_web::fetch::NoNetwork),
        hostname: "iyakki-test".into(),
        data_volatile: false,
        hwcheck: false,
    };
    let id = iyakki_remote::tls::identity(&devmode::tls_dir(env.services.dir()), &[]).unwrap();
    let server = DevServer::start(env.share(), "127.0.0.1:0", &id).unwrap();
    let dm = DevMode::open(env.services.dir());
    Fx { env, dir: d, server, dm }
}

fn admin() -> Policy {
    Role::Admin.template()
}

fn port(f: &Fx) -> u16 {
    f.server.local_addr().port()
}

fn home(f: &Fx, n: &str) -> Home {
    Home::at(&f.dir.join(format!("home-{n}")))
}

/// Pair a computer called `name` acting as `account`.
fn paired(f: &Fx, name: &str, account: &str) -> (Home, DevClient) {
    let h = home(f, name);
    let p = f.dm.start_pairing_as(&admin(), "root", account).unwrap();
    let d = client::pair(&h, "127.0.0.1", port(f), name, &p.code, None, Some(name)).unwrap();
    let c = DevClient::new(&h, d).unwrap();
    (h, c)
}

fn tile_dir(f: &Fx, id: &str, version: &str, text: &str) -> PathBuf {
    let d = f.dir.join(format!("src-{id}"));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("app.json"), format!(r#"{{"id":"{id}","name":"{id} app","version":"{version}","group":"Dev","icon":"⏰"}}"#)).unwrap();
    std::fs::write(d.join("index.html"), format!("<!doctype html><html><head><title>{id}</title></head><body><h1>{text}</h1></body></html>")).unwrap();
    d
}

fn status_of(f: &Fx, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> (u16, String) {
    let r = client::https("127.0.0.1", port(f), Some(f.server.fingerprint()), method, path, headers, body, Duration::from_secs(20)).unwrap();
    (r.status, r.json()["error"].as_str().unwrap_or("").to_string())
}

const SIGNED: [(&str, &str); 7] = [("GET", "/v1/dev/whoami"), ("POST", "/v1/dev/install"), ("POST", "/v1/dev/uninstall"), ("GET", "/v1/dev/list"), ("GET", "/v1/dev/logs"), ("POST", "/v1/dev/open"), ("GET", "/v1/dev/screenshot")];

#[test]
fn everything_is_refused_while_developer_mode_is_off_and_pairing_needs_the_right_code() {
    let f = fx("off");
    // off: even the greeting
    for (m, p) in SIGNED.iter().copied().chain([("GET", "/v1/dev/hello"), ("POST", "/v1/dev/pair")]) {
        let (code, why) = status_of(&f, m, p, &[], b"{}");
        assert_eq!((code, why.as_str()), (403, "developer mode is off on this device"), "{m} {p}");
    }
    assert!(client::hello("127.0.0.1", port(&f)).unwrap_err().contains("off"));
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (fp, info) = client::hello("127.0.0.1", port(&f)).unwrap();
    assert_eq!(fp, f.server.fingerprint(), "the client sees the device's certificate");
    assert_eq!(devmode::cert_fingerprint(f.env.services.dir()).as_deref(), Some(fp.as_str()), "the Settings page shows the same fingerprint");
    assert_eq!((info["name"].as_str(), info["pairing"].as_bool()), (Some("iyakki-test"), Some(false)));
    let h = home(&f, "laptop");
    assert!(client::pair(&h, "127.0.0.1", port(&f), "laptop", "AAAAA-AAAAA", None, None).unwrap_err().contains("no pairing code"));
    let p = f.dm.start_pairing(&admin(), "root").unwrap();
    assert!(client::hello("127.0.0.1", port(&f)).unwrap().1["pairing"].as_bool().unwrap());
    let wrong = if p.code.starts_with('A') { "BBBBB-BBBBB" } else { "AAAAA-AAAAA" };
    assert!(client::pair(&h, "127.0.0.1", port(&f), "laptop", wrong, None, None).unwrap_err().contains("wrong code"));
    assert!(client::pair(&h, "127.0.0.1", port(&f), "laptop", &p.code, Some("00:11"), None).unwrap_err().contains("not 00:11"), "a fingerprint that is not the device's stops the pairing before anything is sent");
    assert!(h.devices().is_empty(), "nothing saved after a failure");
    let d = client::pair(&h, "127.0.0.1", port(&f), "laptop", &p.code.to_lowercase().replace('-', ""), Some(&fp), None).unwrap();
    assert_eq!((d.name.as_str(), d.account.as_str(), d.fingerprint.as_str()), ("iyakki-test", "root", fp.as_str()));
    assert_eq!(f.dm.keys().len(), 1);
    assert!(client::pair(&h, "127.0.0.1", port(&f), "laptop", &p.code, None, None).is_err(), "the code is used up");
    let c = DevClient::new(&h, d).unwrap();
    assert_eq!(c.whoami().unwrap()["account"], "root");
    // switched off again: the paired computer is refused too
    f.dm.disable(&admin(), "root").unwrap();
    assert!(c.whoami().unwrap_err().contains("developer mode is off"));
    assert!(f.env.audit.recent(50).iter().any(|l| l.contains("devmode.pair")), "pairing is in the audit log");
}

#[test]
fn every_endpoint_needs_a_paired_key_and_a_valid_signature_and_a_revoked_key_is_refused() {
    let f = fx("auth");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (_h, c) = paired(&f, "laptop", "root");
    let stranger = iyakki_agent::pkg::new_signing_key().unwrap();
    let fp = f.server.fingerprint().to_string();
    for (m, p) in SIGNED {
        // no signature at all
        assert_eq!(status_of(&f, m, p, &[], b"").0, 401, "{m} {p} unsigned");
        // signed by a key that never paired
        let h = iyakki_agent::devmode::auth::sign_headers(&stranger, m, p, &[], b"", &fp, iyakki_agent::clock::now_unix()).unwrap();
        assert_eq!(status_of(&f, m, p, &h, b"").0, 403, "{m} {p} stranger");
        // a paired key, but the body was changed after signing
        let d = &c.device;
        let k = home(&f, "laptop").key().unwrap();
        let h = iyakki_agent::devmode::auth::sign_headers(&k, m, p, &[], b"original", &d.fingerprint, iyakki_agent::clock::now_unix()).unwrap();
        assert_eq!(status_of(&f, m, p, &h, b"changed").0, 401, "{m} {p} tampered");
        // replayed
        let h = iyakki_agent::devmode::auth::sign_headers(&k, "GET", "/v1/dev/whoami", &[], b"", &d.fingerprint, iyakki_agent::clock::now_unix()).unwrap();
        assert_eq!(status_of(&f, "GET", "/v1/dev/whoami", &h, b"").0, 200);
        assert_eq!(status_of(&f, "GET", "/v1/dev/whoami", &h, b"").0, 401, "a replay is refused");
    }
    // the certificate is pinned: a client expecting another one cannot even connect
    let other = Device { fingerprint: "00:11:22".into(), ..c.device.clone() };
    assert!(DevClient::with_key(other, home(&f, "laptop").key().unwrap()).whoami().unwrap_err().contains("certificate changed"));
    // revoked
    assert!(c.list(false).is_ok());
    f.dm.revoke(&admin(), "root", "laptop").unwrap();
    assert!(c.list(false).unwrap_err().contains("not paired"));
    let refused = f.dm.activity(200).into_iter().filter(|a| a.action == "devmode.auth" && !a.ok).count();
    assert!(refused >= SIGNED.len() * 3, "refusals are in the activity log ({refused})");
}

#[test]
fn install_upgrade_logs_open_screenshot_and_uninstall_go_through_the_store_and_are_audited() {
    let f = fx("install");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (h, c) = paired(&f, "laptop", "root");
    let key = h.key().unwrap();
    let dir = tile_dir(&f, "clock-dev", "1.0.0", "one");
    let v = c.install(&devmode::ops::package_bytes(&dir, &key).unwrap(), false).unwrap();
    assert_eq!((v["kind"].as_str(), v["name"].as_str(), v["level"].as_str(), v["scope"].as_str()), (Some("tile"), Some("clock-dev"), Some("developer"), Some("user/root")));
    let store = iyakki_agent::store::Store::for_services(&f.env.services);
    let rec = store.records().into_values().find(|r| r.name == "clock-dev").unwrap();
    assert_eq!(rec.source, "developer:laptop", "installed by the STORE pipeline with the developer source");
    let list = c.list(false).unwrap();
    assert_eq!(list["items"][0]["trust"], "Developer (local)");
    // an upgrade
    iyakki_agent::store::publishflow::set_version(&dir, "1.0.1").unwrap();
    assert_eq!(c.install(&devmode::ops::package_bytes(&dir, &key).unwrap(), false).unwrap()["version"], "1.0.1");
    // logs: what happened, and the page's console as the shell writes it
    f.dm.app_log("clock-dev", "console.log: tick");
    let l = c.logs("clock-dev", 0).unwrap();
    let text = l["text"].as_str().unwrap();
    assert!(text.contains("installed tile:clock-dev 1.0.0") && text.contains("1.0.1") && text.contains("console.log: tick"), "{text}");
    let next = l["next"].as_u64().unwrap();
    f.dm.app_log("clock-dev", "later line");
    assert!(c.logs("clock-dev", next).unwrap()["text"].as_str().unwrap().contains("later line"), "follow reads only what is new");
    assert!(c.logs("not-mine", 0).is_err(), "only the logs of what this account has");
    // open on the screen
    assert!(c.open("nope").is_err());
    c.open("clock-dev").unwrap();
    assert_eq!(devmode::take_open(f.env.services.dir()), vec!["clock-dev".to_string()], "the shell is asked to open it");
    // screenshot: a shell that is drawing answers
    let svc = f.env.services.clone();
    svc.screen.set_present(true);
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    let painter = std::thread::spawn(move || {
        while !st.load(Ordering::SeqCst) {
            if svc.screen.wanted(SCREEN_USER) {
                svc.screen.serve(SCREEN_USER, || Some(iyakki_agent::screencap::Frame { w: 4, h: 2, rgba: vec![200; 32] }));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let png = c.screenshot().unwrap();
    stop.store(true, Ordering::SeqCst);
    let _ = painter.join();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    // uninstall
    assert_eq!(c.uninstall("tile:clock-dev").unwrap()["version"], "1.0.1");
    assert!(c.list(false).unwrap()["items"].as_array().unwrap().is_empty());
    assert!(c.uninstall("clock-dev").is_err());
    let audit = f.env.audit.recent(200).join("\n");
    for a in ["devmode.install", "devmode.open", "devmode.screenshot", "devmode.uninstall"] {
        assert!(audit.contains(a), "{a} is in the audit log:\n{audit}");
    }
    assert!(audit.contains("laptop (developer computer)"), "the audit names the computer");
    let acts: Vec<String> = f.dm.activity(100).iter().map(|a| a.action.clone()).collect();
    for a in ["dev.install", "dev.list", "dev.logs", "dev.open", "dev.screenshot", "dev.uninstall"] {
        assert!(acts.iter().any(|x| x == a), "{a} in the activity log: {acts:?}");
    }
}

#[test]
fn the_accounts_role_decides_and_drivers_are_refused() {
    let f = fx("roles");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (h, c) = paired(&f, "bobs-laptop", "bob");
    assert_eq!(c.whoami().unwrap()["role"], "user");
    let dir = tile_dir(&f, "bob-app", "1.0.0", "hi");
    let bytes = devmode::ops::package_bytes(&dir, &h.key().unwrap()).unwrap();
    assert!(c.install(&bytes, true).unwrap_err().contains("for everyone"), "a user cannot install for everyone");
    assert_eq!(c.install(&bytes, false).unwrap()["scope"], "user/bob");
    let meta = r#"{"title":"Wifi","ext_kind":"driver","target":"system"}"#;
    let drv = iyakki_agent::pkg::build_generic("extension", "wifi-fw", "1.0.0", "MIT", r#"{"matches":[],"files":[]}"#, Some(meta), None, Some(&h.key().unwrap())).unwrap();
    let (_, root) = paired(&f, "root-laptop", "root");
    assert!(root.install(drv.as_bytes(), false).unwrap_err().contains("driver"), "not even for an administrator");
    // an account that is gone takes its computers with it
    let mut users = f.env.store.lock().unwrap();
    let sess = users.authenticate("root", "rootpass12").unwrap();
    users.delete_user(&sess, "bob").unwrap();
    drop(users);
    assert!(c.whoami().unwrap_err().contains("no longer exists"));
}

#[test]
fn push_watch_reinstalls_on_every_change_and_the_cli_drives_the_same_client() {
    let f = fx("watch");
    f.dm.enable(&admin(), "root", 0).unwrap();
    let (h, c) = paired(&f, "laptop", "root");
    let dir = tile_dir(&f, "live-dev", "1.0.0", "first");
    let key = h.key().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let (st, ln, d2, dev, k2) = (stop.clone(), lines.clone(), dir.clone(), c.device.clone(), key.clone());
    let watcher = std::thread::spawn(move || {
        let c = DevClient::with_key(dev, k2);
        let mut out = |s: &str| ln.lock().unwrap().push(s.to_string());
        cli::push(&c, &d2, &key, cli::Push { watch: true, ..Default::default() }, &mut out, &|| !st.load(Ordering::SeqCst))
    });
    let data = f.env.services.dir().to_path_buf();
    let wait = |what: &dyn Fn() -> bool| {
        let t = Instant::now();
        while !what() && t.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(what(), "timed out: {:?}", lines.lock().unwrap());
    };
    wait(&|| devmode::reload_stamp(&data, "live-dev") >= 1);
    let first = devmode::reload_stamp(&data, "live-dev");
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(dir.join("index.html"), "<!doctype html><html><body><h1>second</h1></body></html>").unwrap();
    wait(&|| devmode::reload_stamp(&data, "live-dev") > first);
    stop.store(true, Ordering::SeqCst);
    watcher.join().unwrap().unwrap();
    let store = iyakki_agent::store::Store::for_services(&f.env.services);
    let content = store.content("tile", "root").into_iter().find(|x| x.name == "live-dev").unwrap();
    assert!(content.payload.contains("second"), "the changed build is what is installed");
    assert!(lines.lock().unwrap().iter().any(|l| l.starts_with("Reinstalled tile:live-dev 1.0.0")));
    // the command line, with the same home
    let mut out = Vec::new();
    let home_arg = h.dir().display().to_string();
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).chain(["--home".to_string(), home_arg.clone()]).collect::<Vec<_>>();
    cli::run(&a(&["list"]), &mut |s| out.push(s.to_string())).unwrap();
    assert!(out.iter().any(|l| l.contains("live-dev") && l.contains("Developer (local)")), "{out:?}");
    cli::run(&a(&["devices"]), &mut |s| out.push(s.to_string())).unwrap();
    assert!(out.iter().any(|l| l.starts_with("* laptop")), "{out:?}");
    cli::run(&a(&["uninstall", "live-dev"]), &mut |s| out.push(s.to_string())).unwrap();
    assert!(out.iter().any(|l| l.starts_with("Removed tile:live-dev")), "{out:?}");
    assert!(cli::run(&a(&["frobnicate"]), &mut |_| {}).unwrap_err().contains("unknown command"));
}
