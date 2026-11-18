//! A person's own layer of the tenant configuration (the lowest layer of [`super::resolve`]): an API to store and read it, for the kinds
//! that allow a person to narrow them (`person_override`). The layer can only narrow what the tenant and the team already configured
//! (the resolver guarantees it); it cannot create an item, cannot hold a secret, is checked by the same schema and safety rules as a
//! tenant item, and every change is one audit line (hashes and field names, no values).
//!
//! The layers live in `<data_dir>/cloud/config/person/<person>.json`, one file per person, written atomically with mode 0600.

use super::api::{emit, ApiErr, Caller, Env, R};
use super::resolve::{resolve, Layer, LayerKind};
use super::safety;
use super::schema::{self, kind_from_url, Ty};
use super::store::Store;
use super::{Doc, Item, DEFAULT_NAME, SCHEMA_VERSION};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// At most this many person items per person.
pub const MAX_PERSON_ITEMS: usize = 50;

fn e(status: u16, code: &'static str, m: &str) -> ApiErr {
    ApiErr { status, code, message: m.into() }
}

/// A file name for a person: letters, digits, `-`, `_` and `.` stay; anything else becomes `%xx`, so two people never share a file.
fn file_name(user: &str) -> String {
    let mut out = String::new();
    for b in user.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02x}"));
        }
    }
    if out.is_empty() {
        out.push_str("%00");
    }
    out.truncate(120);
    format!("{out}.json")
}

fn path(dir: &Path, user: &str) -> PathBuf {
    dir.join("cloud").join("config").join("person").join(file_name(user))
}

/// The person's items. A file that does not read is no layer (the safe reading: nothing narrower than the tenant is lost, nothing wider is gained).
pub fn load(dir: &Path, user: &str) -> Vec<Item> {
    let Ok(t) = std::fs::read_to_string(path(dir, user)) else { return vec![] };
    serde_json::from_str::<Doc>(&t).map(|d| d.items).unwrap_or_default().into_iter().filter(|i| i.namespace.is_none()).collect()
}

fn save(dir: &Path, user: &str, mut items: Vec<Item>) -> Result<(), String> {
    items.sort_by_key(|i| i.key());
    let p = path(dir, user);
    if items.is_empty() {
        return match std::fs::remove_file(&p) {
            Ok(()) => Ok(()),
            Err(x) if x.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(x) => Err(x.to_string()),
        };
    }
    let doc = Doc { schema: SCHEMA_VERSION, items };
    iyakki_schema::write_atomic(&p, serde_json::to_string_pretty(&doc).map_err(|x| x.to_string())?.as_bytes(), 0o600)
}

/// The person layer of one item, for [`Layer`] lists: `(kind, name)` of the item.
pub fn layer_of<'a>(items: &'a [Item], kind: &str, name: &str) -> Option<Layer<'a>> {
    items.iter().find(|i| i.kind == kind && i.name == name).map(|item| Layer { kind: LayerKind::Person, item })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    values: BTreeMap<String, Value>,
    #[serde(default)]
    name: Option<String>,
}

fn member(c: &Caller) -> Result<(), ApiErr> {
    c.tenant_role().map(|_| ()).ok_or_else(|| e(403, "denied", "no access"))
}

fn spec_for(kind: &str) -> Result<&'static schema::KindSpec, ApiErr> {
    let spec = kind_from_url(kind).ok_or_else(|| e(404, "not_found", "no such kind"))?;
    if !spec.person_override {
        return Err(e(400, "invalid_input", "a person cannot set their own layer of this kind"));
    }
    Ok(spec)
}

/// `PUT` the caller's own layer of `kind` (`name` in the body for a collection). Returns what the person ends up with.
pub fn put(env: &Env, c: &Caller, kind: &str, body: Value) -> R {
    member(c)?;
    let spec = spec_for(kind)?;
    let b: Body = serde_json::from_value(body).map_err(|_| e(400, "invalid_input", "send {\"enabled\": bool, \"values\": {...}}; a personal layer holds no secrets"))?;
    let name = b.name.clone().unwrap_or_else(|| DEFAULT_NAME.into());
    let item = Item { kind: spec.id.into(), name: name.clone(), namespace: None, enabled: b.enabled.unwrap_or(false), managed: false, values: b.values };
    // no secrets in a personal layer: a secret field would need the tenant vault and a reference nobody reviews
    if spec.all_fields().iter().any(|f| matches!(f.ty, Ty::SecretRef(_)) && item.values.contains_key(f.name)) {
        return Err(e(400, "invalid_input", "a personal layer holds no secrets"));
    }
    let cfg = Store::new(env.dir).load().map_err(|m| e(503, "unavailable", &m))?;
    let problems = schema::validate(&item, &cfg.all_items());
    if !problems.is_empty() {
        return Err(e(400, "invalid_input", &problems.iter().map(|p| format!("{}: {}", p.field, p.message)).collect::<Vec<_>>().join("; ")));
    }
    // it narrows something that exists; it never creates
    let entries = cfg.entries();
    let above = resolve(spec, &cfg.layers(&entries, spec.id, None, &name));
    if !above.exists {
        return Err(e(409, "not_configured", "the tenant has not configured this, so there is nothing to narrow"));
    }
    let mut items = load(env.dir, c.user);
    let before = items.iter().find(|i| i.key() == item.key()).cloned();
    items.retain(|i| i.key() != item.key());
    items.push(item.clone());
    if items.len() > MAX_PERSON_ITEMS {
        return Err(e(400, "invalid_input", &format!("at most {MAX_PERSON_ITEMS} personal settings")));
    }
    // the same safety rules, on what the person would end up with
    let mut layers = cfg.layers(&entries, spec.id, None, &name);
    layers.push(Layer { kind: LayerKind::Person, item: &item });
    let eff = resolve(spec, &layers);
    let warns = safety::evaluate(&safety::Ctx { spec, item: &item, eff: &eff, cfg: &cfg, roles: c.roles, meta: &Store::new(env.dir).meta(), now: super::now_secs() });
    if let Some(w) = warns.iter().find(|w| w.severity == "block") {
        return Err(e(400, "invalid_input", &format!("blocked by safety rules: {} ({}): {}", w.code, w.field, w.text)));
    }
    save(env.dir, c.user, items).map_err(|m| e(409, "io", &m))?;
    let fields: Vec<&str> = item.values.keys().map(String::as_str).collect();
    emit(env, c.user, "tenant.config.person.put", &format!("kind={} name={} before={} after={} fields={}", spec.id, name, before.map_or("none".to_string(), |x| x.hash()), item.hash(), fields.join(",")));
    Ok(json!({"ok": true, "effective": eff.values, "enabled": eff.enabled}))
}

/// `GET` the caller's own layer of `kind` (all names of it). Another person's layer is never reachable: the caller is the key.
pub fn get(env: &Env, c: &Caller, kind: &str) -> R {
    member(c)?;
    let spec = spec_for(kind)?;
    let items: Vec<Value> = load(env.dir, c.user).into_iter().filter(|i| i.kind == spec.id).map(|i| json!({"name": i.name, "enabled": i.enabled, "values": i.values, "hash": i.hash()})).collect();
    Ok(json!({"kind": spec.id, "items": items}))
}

/// `DELETE` the caller's own layer of `kind` and `name` (`default` for a single value).
pub fn delete(env: &Env, c: &Caller, kind: &str, name: Option<&str>) -> R {
    member(c)?;
    let spec = spec_for(kind)?;
    let name = name.unwrap_or(DEFAULT_NAME);
    let mut items = load(env.dir, c.user);
    let before = items.iter().find(|i| i.kind == spec.id && i.name == name).cloned().ok_or_else(|| e(404, "not_found", "you have no personal setting here"))?;
    items.retain(|i| !(i.kind == spec.id && i.name == name));
    save(env.dir, c.user, items).map_err(|m| e(409, "io", &m))?;
    emit(env, c.user, "tenant.config.person.delete", &format!("kind={} name={} before={} after=none", spec.id, name, before.hash()));
    Ok(json!({"ok": true}))
}
