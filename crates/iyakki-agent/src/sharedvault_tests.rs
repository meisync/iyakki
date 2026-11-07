//! The shared vault's provider: default built-in, an external system never leaves a secret byte on the local volume, fail closed,
//! damaged or newer settings, and no credential in the audit trail. The external system is a fake; nothing touches a network.

use crate::cloudkeys::MemberKeys;
use crate::cloudroles;
use crate::cloudvaultd::{Client, Principal, Scope};
use crate::sharedvault::*;
use iyakki_proxy::{Upstream, UpstreamResponse};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const TOKEN: &str = "hvs.PLANTED-token-4c9e1a";
const VALUE: &str = "sk-PLANTED-value-77aa31";

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-sv-{tag}-{}", crate::cloudkeys::hex(&crate::cloudkeys::random::<6>().unwrap())));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Up,
    Down,
    Reject,
}

struct Fake {
    mode: Mutex<Mode>,
    data: Mutex<BTreeMap<String, String>>,
    seen_tokens: Mutex<Vec<String>>,
}

impl Fake {
    fn new() -> Arc<Fake> {
        Arc::new(Fake { mode: Mutex::new(Mode::Up), data: Mutex::new(BTreeMap::new()), seen_tokens: Mutex::new(vec![]) })
    }
}

impl Upstream for Fake {
    fn request(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
        if let Some((_, t)) = headers.iter().find(|(k, _)| k == "X-Vault-Token") {
            self.seen_tokens.lock().unwrap().push(t.clone());
        }
        match *self.mode.lock().unwrap() {
            Mode::Down => return Err(format!("connect to {url} failed with {TOKEN}")),
            Mode::Reject => return Ok(UpstreamResponse { status: 403, body: vec![] }),
            Mode::Up => {}
        }
        let rest = url.split("/v1/").nth(1).unwrap_or("");
        let (kind, path) = rest.split_once('/').unwrap_or(("", ""));
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let ok = |v: serde_json::Value| Ok(UpstreamResponse { status: 200, body: v.to_string().into_bytes() });
        let none = Ok(UpstreamResponse { status: 404, body: vec![] });
        let mut d = self.data.lock().unwrap();
        match (method, kind) {
            ("POST", "secret") => {
                let v: serde_json::Value = serde_json::from_slice(body).unwrap();
                d.insert(path.trim_start_matches("data/").into(), v["data"]["value"].as_str().unwrap().into());
                ok(serde_json::json!({}))
            }
            _ => {
                let key = path.trim_start_matches("data/").trim_start_matches("metadata/");
                match (method, path.split('/').next().unwrap_or("")) {
                    ("GET", "data") => d.get(key).map_or(none, |v| ok(serde_json::json!({"data": {"data": {"value": v}}}))),
                    ("GET", "metadata") if query == "list=true" => {
                        let pre = format!("{key}/");
                        let keys: Vec<String> = d.keys().filter_map(|k| k.strip_prefix(&pre).map(String::from)).collect();
                        if keys.is_empty() { none } else { ok(serde_json::json!({"data": {"keys": keys}})) }
                    }
                    ("DELETE", "metadata") => {
                        d.remove(key);
                        Ok(UpstreamResponse { status: 204, body: vec![] })
                    }
                    _ => Ok(UpstreamResponse { status: 400, body: vec![] }),
                }
            }
        }
    }
}

fn external() -> Setting {
    Setting {
        v: FILE_VERSION,
        provider: Provider::External,
        external: Some(ExternalRef { address: "https://kms.example.invalid:8200".into(), mount: "secret".into(), path: "iyakki/acme".into(), namespace: None, token_secret: "shared-vault-token".into() }),
    }
}

fn dummy_client() -> Client {
    Client { host: "127.0.0.1".into(), port: 1, fingerprint: None, who: Principal::Member { tenant: "acme".into(), user: "alice".into() }, keys: MemberKeys::from_seed([1; 32]) }
}

fn wiring(d: &Path, up: Arc<Fake>, builtin: bool) -> Wiring {
    Wiring { data_dir: d.into(), actor: "alice".into(), builtin: builtin.then(dummy_client), upstream: up, token: Box::new(|n| if n == "shared-vault-token" { Ok(TOKEN.to_string().into()) } else { Err("no such secret".into()) }) }
}

fn all_files(d: &Path) -> String {
    let mut out = String::new();
    fn walk(d: &Path, out: &mut String) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push_str(&String::from_utf8_lossy(&std::fs::read(&p).unwrap()));
            }
        }
    }
    walk(d, &mut out);
    out
}

fn audit(d: &Path) -> String {
    std::fs::read_to_string(cloudroles::audit_path(d)).unwrap_or_default()
}

#[test]
fn the_provider_is_builtin_when_nothing_is_set_and_the_label_says_not_reviewed() {
    let d = dir("default");
    let s = Setting::load(&d).unwrap();
    assert_eq!(s.provider, Provider::Builtin);
    let v = SharedVault::open(wiring(&d, Fake::new(), true)).unwrap();
    assert_eq!(v.provider(), Provider::Builtin);
    assert!(page_line(&d).contains("built-in") && page_line(&d).contains("not externally reviewed"));
    // builtin with no built-in client is an error, not a switch to anything else
    assert!(SharedVault::open(wiring(&d, Fake::new(), false)).is_err());
}

#[test]
fn an_external_provider_keeps_every_secret_byte_off_the_local_volume() {
    let d = dir("nolocal");
    external().save(&d, "alice").unwrap();
    let up = Fake::new();
    let v = SharedVault::open(wiring(&d, up.clone(), false)).unwrap();
    assert_eq!(v.provider(), Provider::External);
    let t = Scope::Tenant;
    v.put(&t, "openai", VALUE.as_bytes()).unwrap();
    v.put(&Scope::Namespace("dev".into()), "db", b"pw-two").unwrap();
    assert_eq!(v.get(&t, "openai").unwrap(), VALUE.as_bytes());
    assert_eq!(v.list(&t).unwrap(), vec!["openai".to_string()]);
    assert!(up.data.lock().unwrap().contains_key("iyakki/acme/tenant/openai"));
    assert!(up.data.lock().unwrap().contains_key("iyakki/acme/namespace.dev/db"));
    v.delete(&t, "openai").unwrap();
    assert!(v.get(&t, "openai").is_err());
    let local = all_files(&d);
    assert!(!local.contains(VALUE) && !local.contains("pw-two"), "no value on the local volume");
    assert!(!local.contains(TOKEN), "no token on the local volume");
    assert!(local.contains("shared-vault-token"), "only the NAME of the token secret is kept");
    // the token came from the vault by name for every call
    assert!(up.seen_tokens.lock().unwrap().iter().all(|t| t == TOKEN));
    // non-text is refused plainly
    assert!(v.put(&t, "bin", &[0xff, 0xfe]).unwrap_err().contains("not text"));
}

#[test]
fn an_unreachable_or_refusing_external_system_fails_closed_with_a_plain_message() {
    let d = dir("closed");
    external().save(&d, "alice").unwrap();
    let up = Fake::new();
    let v = SharedVault::open(wiring(&d, up.clone(), true)).unwrap();
    v.put(&Scope::Tenant, "k", b"v1").unwrap();
    *up.mode.lock().unwrap() = Mode::Down;
    let e = v.get(&Scope::Tenant, "k").unwrap_err();
    assert!(e.contains("could not be reached") && e.contains("built-in store was not used"), "{e}");
    assert!(!e.contains(TOKEN) && !e.contains("example.invalid"), "the transport's text is dropped: {e}");
    assert!(v.put(&Scope::Tenant, "k2", b"v2").is_err());
    *up.mode.lock().unwrap() = Mode::Reject;
    assert!(v.get(&Scope::Tenant, "k").unwrap_err().contains("refused the access secret"));
    // a token secret that is not in the vault: closed too, and the external system is never asked
    let n = up.seen_tokens.lock().unwrap().len();
    let mut w = wiring(&d, up.clone(), true);
    w.token = Box::new(|_| Err("locked".into()));
    let v2 = SharedVault::open(w).unwrap();
    assert!(v2.get(&Scope::Tenant, "k").unwrap_err().contains("not available in the vault"));
    assert_eq!(up.seen_tokens.lock().unwrap().len(), n);
    assert!(!all_files(&d).contains("v1") || !all_files(&d).contains("v2"));
}

#[test]
fn a_damaged_or_newer_setting_closes_the_vault_and_never_means_builtin() {
    let d = dir("bad");
    let file = Setting::path(&d);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    for text in [
        "{ not json",
        r#"{"v":2,"provider":"builtin"}"#,
        r#"{"v":1,"provider":"kms"}"#,
        r#"{"v":1,"provider":"builtin","future_field":1}"#,
        r#"{"provider":"builtin"}"#,
        r#"{"v":1,"provider":"external"}"#,
        r#"{"v":1,"provider":"external","external":{"address":"http://plain.example","path":"a","token_secret":"t"}}"#,
        r#"{"v":1,"provider":"external","external":{"address":"https://k.example","path":"a/../b","token_secret":"t"}}"#,
        r#"{"v":1,"provider":"external","external":{"address":"https://k.example","path":"a","token_secret":"hvs.abc:inline"}}"#,
        r#"{"v":1,"provider":"external","external":{"address":"https://k.example","path":"a","token_secret":"t","token":"inline"}}"#,
    ] {
        std::fs::write(&file, text).unwrap();
        let e = Setting::load(&d).unwrap_err();
        assert!(e.contains("stays closed") || e.contains("cannot be used") || e.contains("needs") || e.contains("https"), "{text}: {e}");
        assert!(SharedVault::open(wiring(&d, Fake::new(), true)).is_err(), "{text}");
        assert!(page_line(&d).contains("stays closed") && page_line(&d).contains("not externally reviewed"), "{text}");
    }
    std::fs::remove_file(&file).unwrap();
    assert_eq!(Setting::load(&d).unwrap().provider, Provider::Builtin);
}

#[test]
fn the_setting_round_trips_and_the_audit_names_provider_and_action_but_never_a_value_or_credential() {
    let d = dir("audit");
    external().save(&d, "alice").unwrap();
    assert_eq!(Setting::load(&d).unwrap(), external());
    let up = Fake::new();
    let v = SharedVault::open(wiring(&d, up.clone(), false)).unwrap();
    v.put(&Scope::Tenant, "openai", VALUE.as_bytes()).unwrap();
    *up.mode.lock().unwrap() = Mode::Down;
    let _ = v.get(&Scope::Tenant, "openai");
    let _ = v.put(&Scope::Tenant, "sk-this is not a name", b"x");
    let a = audit(&d);
    assert!(a.contains("provider set to external"));
    assert!(a.contains("provider external put tenant openai by alice: ok"), "{a}");
    assert!(a.contains("provider external get tenant openai by alice: failed: unreachable"), "{a}");
    assert!(a.contains("not externally reviewed"));
    assert!(!a.contains(VALUE) && !a.contains(TOKEN) && !a.contains("example.invalid") && !a.contains("sk-this"), "{a}");
    let mode = std::fs::metadata(Setting::path(&d)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(mode.permissions().mode() & 0o777, 0o600);
    // a setting with an inline credential cannot even be saved
    let mut bad = external();
    bad.external.as_mut().unwrap().token_secret = "hvs.inline:token".into();
    assert!(bad.save(&d, "alice").is_err());
}
