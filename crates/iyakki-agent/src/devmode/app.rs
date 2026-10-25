//! `config` scope, apps: look at and change the saved data of a tile app that developer mode installed (`iyakki dev app info|permissions|
//! data|clear-data|reset`). Only apps from a paired developer computer (labelled Developer (local)) and dev apps are reachable:
//! the saved data of a store app is the person's own and is not for a computer to read. The data is the app's `iyakki.storage`
//! keys; an app that is open keeps its own copy until it is opened again.

use std::path::PathBuf;

use iyakki_sec::policy::Policy;
use serde_json::{json, Value};

use crate::services::Services;
use crate::store::tile::TileStorage;
use crate::store::tilehub::{self, TileEntry};
use crate::store::Store;

fn entry(svc: &Services, policy: &Policy, account: &str, app: &str) -> Result<TileEntry, String> {
    if !crate::skills::valid_skill_name(app) {
        return Err("that is not an app name".into());
    }
    let e = tilehub::installed(svc, policy, account).into_iter().find(|t| t.name == app).ok_or_else(|| format!("{app} is not a tile app installed for {account}"))?;
    if !(e.is_developer() || e.dev) {
        return Err(format!("{app} did not come from developer mode: its saved data is not available to a computer"));
    }
    Ok(e)
}

fn storage_path(svc: &Services, account: &str, e: &TileEntry) -> PathBuf {
    if e.dev { svc.dir().join("appdata").join("tile-dev").join(account).join(format!("{}.json", e.name)) } else { crate::store::tile::storage_file(&Store::for_services(svc), &e.scope, &e.name, account) }
}

fn valid_key(k: &str) -> bool {
    !k.is_empty() && k.len() <= 128 && !k.chars().any(|c| c.is_control())
}

pub fn info(svc: &Services, policy: &Policy, account: &str, app: &str) -> Result<Value, String> {
    let e = entry(svc, policy, account, app)?;
    let st = TileStorage::open(&storage_path(svc, account, &e));
    Ok(json!({ "name": e.name, "title": e.title, "version": e.version, "publisher": e.publisher, "trust": e.trust, "group": e.group, "dev": e.dev, "scope": format!("{:?}", e.scope), "permissions": e.caps.permissions(), "storage_keys": st.list().len(), "storage_bytes": st.used() }))
}

pub fn permissions(svc: &Services, policy: &Policy, account: &str, app: &str) -> Result<Value, String> {
    let e = entry(svc, policy, account, app)?;
    Ok(json!({ "name": e.name, "declared": e.caps.permissions(), "note": "an app holds exactly what its manifest declares; to change it, edit the manifest and install again" }))
}

/// All keys with sizes, or one value.
pub fn data(svc: &Services, policy: &Policy, account: &str, app: &str, key: &str) -> Result<Value, String> {
    let e = entry(svc, policy, account, app)?;
    let st = TileStorage::open(&storage_path(svc, account, &e));
    if key.is_empty() {
        let keys: Vec<Value> = st.list().into_iter().map(|k| json!({ "key": k, "bytes": st.get(&k).map_or(0, |v| v.len()) })).collect();
        return Ok(json!({ "keys": keys, "bytes": st.used() }));
    }
    st.get(key).map(|v| json!({ "key": key, "value": v })).ok_or_else(|| format!("{app} has no saved key {key}"))
}

pub fn set_data(svc: &Services, policy: &Policy, account: &str, app: &str, key: &str, value: &str) -> Result<Value, String> {
    let e = entry(svc, policy, account, app)?;
    if !valid_key(key) {
        return Err("a key is 1 to 128 characters without control characters".into());
    }
    let mut st = TileStorage::open(&storage_path(svc, account, &e));
    st.set(key, value)?;
    Ok(json!({ "key": key, "bytes": value.len() }))
}

pub fn remove_data(svc: &Services, policy: &Policy, account: &str, app: &str, key: &str) -> Result<Value, String> {
    let e = entry(svc, policy, account, app)?;
    let mut st = TileStorage::open(&storage_path(svc, account, &e));
    st.remove(key)?;
    Ok(json!({ "removed": key }))
}

/// Delete all saved data of the app for this account.
pub fn clear_data(svc: &Services, policy: &Policy, account: &str, app: &str) -> Result<Value, String> {
    let e = entry(svc, policy, account, app)?;
    let p = storage_path(svc, account, &e);
    let had = TileStorage::open(&p).list().len();
    match std::fs::remove_file(&p) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.to_string()),
    }
    Ok(json!({ "cleared_keys": had }))
}

/// Saved data and the app's developer log both start again (the app itself stays installed).
pub fn reset(svc: &Services, dm: &super::DevMode, policy: &Policy, account: &str, app: &str) -> Result<Value, String> {
    let v = clear_data(svc, policy, account, app)?;
    let _ = std::fs::remove_file(dm.dir().join("logs").join(format!("{app}.log")));
    Ok(v)
}
