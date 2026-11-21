//! `iyakki vault secret` and the snapshot keyring over the shared vault follow the provider setting. The external system is a fake KV
//! version 2 store; nothing touches a network.

use crate::cloudsnapshot::KeySource;
use crate::sharedvault::{ExternalRef, Provider, Setting, FILE_VERSION};
use crate::sharedvault_cli::{open_vault, secret, Deps, SharedKeyring};
use iyakki_proxy::{Upstream, UpstreamResponse};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const TOKEN: &str = "hvs.PLANTED-cli-token-91";

#[derive(Default)]
struct Kv {
    data: Mutex<BTreeMap<String, String>>,
}

impl Upstream for Kv {
    fn request(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
        if !headers.iter().any(|(k, v)| k == "X-Vault-Token" && v == TOKEN) {
            return Ok(UpstreamResponse { status: 403, body: vec![] });
        }
        let rest = url.split("/v1/secret/").nth(1).unwrap_or("");
        let (kind, path) = rest.split_once('/').unwrap_or(("", ""));
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let mut d = self.data.lock().unwrap();
        let ok = |v: serde_json::Value| Ok(UpstreamResponse { status: 200, body: v.to_string().into_bytes() });
        match (method, kind) {
            ("POST", "data") => {
                let v: serde_json::Value = serde_json::from_slice(body).unwrap();
                d.insert(path.into(), v["data"]["value"].as_str().unwrap().into());
                ok(serde_json::json!({}))
            }
            ("GET", "data") => d.get(path).map_or(Ok(UpstreamResponse { status: 404, body: vec![] }), |v| ok(serde_json::json!({"data": {"data": {"value": v}}}))),
            ("GET", "metadata") if query == "list=true" => {
                let pre = format!("{path}/");
                let keys: Vec<String> = d.keys().filter_map(|k| k.strip_prefix(&pre).map(String::from)).collect();
                ok(serde_json::json!({"data": {"keys": keys}}))
            }
            ("DELETE", "metadata") => {
                d.remove(path);
                Ok(UpstreamResponse { status: 204, body: vec![] })
            }
            _ => Ok(UpstreamResponse { status: 400, body: vec![] }),
        }
    }
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-svc-{tag}-{}", crate::cloudkeys::hex(&crate::cloudkeys::random::<6>().unwrap())));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn external_in(d: &std::path::Path) {
    let s = Setting { v: FILE_VERSION, provider: Provider::External, external: Some(ExternalRef { address: "https://kv.example.invalid".into(), mount: "secret".into(), path: "iyakki/acme".into(), namespace: None, token_secret: "shared-vault-token".into() }) };
    s.save(d, "alice").unwrap();
}

fn deps(kv: &Arc<Kv>, token: bool) -> Deps {
    Deps { upstream: kv.clone(), builtin: None, token: token.then(|| TOKEN.to_string().into()), actor: "alice".into() }
}

fn words(a: &str) -> Vec<String> {
    a.split_whitespace().map(String::from).collect()
}

fn files_text(d: &std::path::Path) -> String {
    let mut out = String::new();
    for e in std::fs::read_dir(d).unwrap().flatten() {
        let p = e.path();
        if p.is_file() {
            out.push_str(&String::from_utf8_lossy(&std::fs::read(&p).unwrap()));
        } else if p.is_dir() {
            out.push_str(&files_text(&p));
        }
    }
    out
}

#[test]
fn the_vault_command_follows_an_external_provider_keeps_no_secret_bytes_locally_and_never_prints_a_value() {
    let d = dir("cmd");
    let data = d.join("data");
    std::fs::create_dir_all(&data).unwrap();
    external_in(&data);
    let kv = Arc::new(Kv::default());
    let vf = d.join("value");
    std::fs::write(&vf, "sk-PLANTED-value-5521").unwrap();
    let base = format!("--data {} --scope tenant --as alice", data.display());
    let mut said = Vec::new();
    secret(&words(&format!("put {base} --name api-key --value-file {}", vf.display())), deps(&kv, true), &mut |l| said.push(l.to_string())).unwrap();
    secret(&words(&format!("list {base}")), deps(&kv, true), &mut |l| said.push(l.to_string())).unwrap();
    assert!(said.iter().any(|l| l == "api-key"), "{said:?}");
    let of = d.join("out");
    secret(&words(&format!("get {base} --name api-key --out {}", of.display())), deps(&kv, true), &mut |l| said.push(l.to_string())).unwrap();
    assert_eq!(std::fs::read_to_string(&of).unwrap(), "sk-PLANTED-value-5521");
    assert!(said.iter().all(|l| !l.contains("sk-PLANTED") && !l.contains(TOKEN)), "{said:?}");
    assert_eq!(kv.data.lock().unwrap().len(), 1, "the value went to the external system");
    // the data folder holds the setting and the audit trail, no value and no token
    let local = files_text(&data);
    assert!(!local.contains("sk-PLANTED") && !local.contains(TOKEN));
    assert!(local.contains("provider external"));
    secret(&words(&format!("delete {base} --name api-key")), deps(&kv, true), &mut |_| {}).unwrap();
    assert!(kv.data.lock().unwrap().is_empty());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn without_the_token_or_a_value_file_or_with_a_damaged_setting_nothing_happens_and_nothing_falls_back_to_the_built_in_store() {
    let d = dir("closed");
    let data = d.join("data");
    std::fs::create_dir_all(&data).unwrap();
    external_in(&data);
    let kv = Arc::new(Kv::default());
    let base = format!("--data {} --scope tenant", data.display());
    let e = secret(&words(&format!("list {base}")), deps(&kv, false), &mut |_| {}).unwrap_err();
    assert!(e.contains("not available") && e.contains("not used instead"), "{e}");
    assert!(secret(&words(&format!("put {base} --name x")), deps(&kv, true), &mut |_| {}).unwrap_err().contains("--value-file"));
    assert!(secret(&words("get --scope tenant"), deps(&kv, true), &mut |_| {}).is_err());
    assert!(secret(&words(&format!("shout {base}")), deps(&kv, true), &mut |_| {}).unwrap_err().contains("usage"));
    assert!(secret(&words("list --data x --scope nonsense"), deps(&kv, true), &mut |_| {}).is_err());
    std::fs::write(data.join("cloud").join("shared-vault.json"), "{broken").unwrap();
    assert!(open_vault(&data, deps(&kv, true)).is_err());
    assert!(kv.data.lock().unwrap().is_empty());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn the_snapshot_keyring_lives_in_the_external_system_as_text_and_rotates_and_opens_old_keys() {
    let d = dir("ring");
    external_in(&d);
    let kv = Arc::new(Kv::default());
    let v = open_vault(&d, deps(&kv, true)).unwrap();
    let ring = SharedKeyring { vault: &v };
    assert!(ring.current().unwrap_err().contains("key-rotate"));
    let first = ring.rotate().unwrap();
    assert_eq!(ring.current().unwrap().id, first.id);
    let second = ring.rotate().unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(ring.current().unwrap().to_hex(), second.to_hex());
    assert_eq!(ring.get(&first.id).unwrap().to_hex(), first.to_hex(), "the older key still opens old snapshots");
    assert!(ring.get("../x").is_err());
    assert!(kv.data.lock().unwrap().values().any(|v| v.len() == 64), "stored as 64 hex digits of text");
    assert!(!files_text(&d).contains(&first.to_hex()), "no key bytes on the local volume");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn the_snapshot_command_with_an_external_shared_vault_never_goes_to_the_built_in_service() {
    let d = dir("snap");
    external_in(&d);
    std::fs::create_dir_all(d.join("cloud").join("files")).unwrap();
    // no token file: the command stops with the plain message, and asks for no built-in vault key either
    let mut said = Vec::new();
    let a = words(&format!("create --data {} --keys vault", d.display()));
    let e = crate::cloudsnapshot::cli(&a, &mut |l| said.push(l.to_string())).unwrap_err();
    assert!(e.contains("not available") && e.contains("not used instead"), "{e}");
    let e = crate::cloudsnapshot::cli(&words(&format!("key-rotate --data {} --keys vault", d.display())), &mut |_| {}).unwrap_err();
    assert!(e.contains("not used instead") || e.contains("refused the key"), "{e}");
    // with the built-in setting the old message about the vault key stays
    let b = dir("snap-builtin");
    let e = crate::cloudsnapshot::cli(&words(&format!("create --data {} --keys vault", b.display())), &mut |_| {}).unwrap_err();
    assert!(e.contains("--vault-key"), "{e}");
    std::fs::remove_dir_all(&d).ok();
    std::fs::remove_dir_all(&b).ok();
}
