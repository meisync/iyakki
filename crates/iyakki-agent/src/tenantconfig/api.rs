//! The operations behind `/api/v1/tenant/config...`, independent of the web service (it maps [`ApiErr`] to its error form).
//!
//! * Read: every tenant member sees which items exist and their state; the values only when the kind allows the member's role
//!   (a team's own people see their team's entries; another team's entries are not there for them).
//! * Write: tenant admins and owners; a team's own admin writes the team's entries of the kinds that allow it. An item the operator's
//!   file marks `managed` is read-only here (409).
//! * Secrets: `secrets: {field: value}` in a PUT goes to the tenant's vault and the config keeps the reference. No call returns a
//!   secret; a secret field reads back as `[set]`, `[not set]` or `[vault locked]`.
//! * Safety (`safety.rs`): every PUT and dry run returns `warnings`; a `block` refuses, a `warn` must be listed in `ack_warnings`.
//! * Every change writes one audit line: who, the item, hashes before and after, the names of the fields that changed, the warning
//!   codes and the acknowledged ones. No values.

use super::catalog;
use super::resolve::{resolve, state_of, Effective, State};
use super::safety::{self, Warning};
use super::schema::{self, default_ref, kind_from_url, KindSpec, Problem, SecretRef, Ty};
use super::store::{meta_key, Config, Entry, Source, Store};
use super::vault::{SecretState, SecretStore};
use super::{Doc, Item, DEFAULT_NAME, SCHEMA_VERSION};
use crate::cloudroles::{Change, NsRole, Roles, TenantRole};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

pub trait ConfigAudit: Send + Sync {
    fn record(&self, who: &str, event: &str, detail: &str);
}

/// Discards (for callers that have no audit sink; the tenant's own audit file is written anyway).
pub struct NoAudit;
impl ConfigAudit for NoAudit {
    fn record(&self, _: &str, _: &str, _: &str) {}
}

pub struct Env<'a> {
    pub dir: &'a Path,
    pub vault: Arc<dyn SecretStore>,
    pub audit: &'a dyn ConfigAudit,
}

pub struct Caller<'a> {
    pub user: &'a str,
    pub roles: Option<&'a Roles>,
}

#[derive(Debug, PartialEq)]
pub struct ApiErr {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

fn e(status: u16, code: &'static str, m: &str) -> ApiErr {
    ApiErr { status, code, message: m.into() }
}

pub type R = Result<Value, ApiErr>;

/// The kinds a team admin may add with a key of the team's own.
const TEAM_KINDS: &[&str] = &["agent.claude", "agent.copilot", "agent.other", "models.provider", "models.local", "integration.apikey"];

/// The audit line of an event (also appended to the tenant's own trail).
pub fn emit(env: &Env, who: &str, event: &str, detail: &str) {
    let clean: String = detail.chars().filter(|c| !c.is_control()).collect();
    env.audit.record(who, event, &clean);
    let _ = crate::cloudroles::append_audit(&crate::cloudroles::audit_path(env.dir), super::now_secs(), &Change(format!("{event} by={who} {clean}")));
}

// ------------------------------------------------------------------------------------------------------------------ who may

impl Caller<'_> {
    pub fn tenant_role(&self) -> Option<TenantRole> {
        self.roles.and_then(|r| r.tenant_role(self.user))
    }
    fn is_admin(&self) -> bool {
        self.tenant_role().is_some_and(|t| t >= TenantRole::Admin)
    }
    fn ns_role(&self, ns: &str) -> Option<NsRole> {
        self.roles.and_then(|r| r.ns_role(ns, self.user))
    }
    /// 0 viewer, 1 editor, 2 admin of the team, 3 tenant admin; -1 nothing.
    pub fn rank(&self, ns: &str) -> i32 {
        let n = match self.ns_role(ns) {
            Some(NsRole::Viewer) => 0,
            Some(NsRole::Editor) => 1,
            Some(NsRole::Admin) => 2,
            None => -1,
        };
        if self.is_admin() {
            n.max(3)
        } else {
            n
        }
    }
    fn sees(&self, ns: Option<&str>) -> bool {
        match ns {
            None => self.tenant_role().is_some(),
            Some(n) => self.is_admin() || self.ns_role(n).is_some(),
        }
    }
    fn writes(&self, spec: &KindSpec, ns: Option<&str>) -> bool {
        self.tenant_role().is_some_and(|t| t >= spec.write) || (spec.team_level && ns.is_some_and(|n| self.ns_role(n) == Some(NsRole::Admin)))
    }
    fn sees_values(&self, spec: &KindSpec, ns: Option<&str>, eff: &Effective) -> bool {
        if self.tenant_role().is_some_and(|t| t >= spec.read_values) {
            return true;
        }
        let Some(n) = ns else { return false };
        let need = eff.values.get("read_role").and_then(|v| v.as_str()).and_then(|s| schema::ROLES.iter().position(|r| *r == s)).map_or(2, |i| i as i32);
        let need = if spec.id == "k3s.cluster" { need } else { 2 };
        self.rank(n) >= need
    }
}

fn need_member(c: &Caller) -> Result<TenantRole, ApiErr> {
    c.tenant_role().ok_or_else(|| e(403, "denied", "no access"))
}

fn need_admin(c: &Caller) -> Result<(), ApiErr> {
    match need_member(c)? {
        t if t >= TenantRole::Admin => Ok(()),
        _ => Err(e(403, "denied", "tenant administrators only")),
    }
}

// ------------------------------------------------------------------------------------------------------------------ views

const SET: &str = "[set]";
const NOT_SET: &str = "[not set]";
const LOCKED: &str = "[vault locked]";

fn show_values(spec: &KindSpec, values: &BTreeMap<String, Value>, vault: &dyn SecretStore) -> Value {
    let mut m = serde_json::Map::new();
    for (k, v) in values {
        let secret = spec.field(k).is_some_and(|f| matches!(f.ty, Ty::SecretRef(_)));
        if !secret {
            m.insert(k.clone(), v.clone());
            continue;
        }
        let s = match v.as_str().and_then(SecretRef::parse).map(|r| vault.state(&r)) {
            Some(SecretState::Set) => SET,
            Some(SecretState::Locked) => LOCKED,
            _ => NOT_SET,
        };
        m.insert(k.clone(), json!(s));
    }
    Value::Object(m)
}

/// The warnings of an item as it would stand (`item`), with the rest of the configuration as it is.
fn warnings_for(env: &Env, cfg: &Config, spec: &'static KindSpec, item: &Item, roles: Option<&Roles>, now: u64) -> Vec<Warning> {
    let mut c2 = cfg.clone();
    c2.local.retain(|i| i.key() != item.key());
    c2.local.push(item.clone());
    let entries = c2.entries();
    let eff = resolve(spec, &c2.layers(&entries, spec.id, item.namespace.as_deref(), &item.name));
    safety::evaluate(&safety::Ctx { spec, item, eff: &eff, cfg: &c2, roles, meta: &Store::new(env.dir).meta(), now })
}

fn describe_entry(env: &Env, c: &Caller, cfg: &Config, entries: &[Entry], en: &Entry) -> Option<Value> {
    let it = &en.item;
    let spec = schema::kind(&it.kind)?;
    let ns = it.namespace.as_deref();
    if !c.sees(ns) {
        return None;
    }
    let layers = cfg.layers(entries, &it.kind, ns, &it.name);
    let eff = resolve(spec, &layers);
    let (mut st, mut reasons) = state_of(spec, &eff, env.vault.as_ref());
    let values_visible = c.sees_values(spec, ns, &eff);
    let meta = Store::new(env.dir).meta();
    let warns = safety::evaluate(&safety::Ctx { spec, item: it, eff: &eff, cfg, roles: c.roles, meta: &meta, now: super::now_secs() });
    let blocks: Vec<&Warning> = warns.iter().filter(|w| w.severity == "block").collect();
    if st == State::Ready && !blocks.is_empty() {
        // a block rule holds for every item, also one the operator's file provides: it is never silently applied
        st = if blocks.iter().all(|w| w.code == "egress.domain.blocked") { State::Disabled } else { State::Invalid };
        reasons.extend(blocks.iter().map(|w| format!("{}: {}", w.code, w.text)));
    }
    let managed = en.source == Source::File && cfg.is_managed(&it.kind, ns, &it.name);
    let write = c.writes(spec, ns) && !managed;
    let mut v = json!({
        "kind": it.kind, "name": it.name, "namespace": it.namespace, "enabled": it.enabled, "managed": managed,
        "source": en.source.name(), "state": st.name(), "reasons": reasons,
        "can": {"read": true, "read_values": values_visible, "write": write, "use": true},
        "hash": it.hash(), "severity": safety::highest(&warns),
    });
    if values_visible {
        v["values"] = show_values(spec, &it.values, env.vault.as_ref());
        v["effective"] = show_values(spec, &eff.values, env.vault.as_ref());
        v["warnings"] = json!(warns.iter().map(|w| w.json()).collect::<Vec<_>>());
    }
    if let Some(m) = meta.get(&meta_key(&it.kind, ns, &it.name)) {
        v["last_rotated"] = json!(m.last_rotated);
        v["last_test"] = json!(m.test.map(|(at, ok, status)| json!({"at": at, "ok": ok, "status": status})));
    }
    if it.kind == "models.local" {
        let t = meta.get(&meta_key(&it.kind, ns, &it.name)).and_then(|m| m.test);
        v["health"] = json!(super::fairshare::health_of(t, super::now_secs(), 900));
    }
    if it.kind == "k3s.cluster" {
        v["reachability"] = json!("unknown");
        // who may approve a deploy here, expanded to people: for team admins and tenant admins only
        if let (Some(team), Some(r)) = (ns, c.roles) {
            if c.rank(team) >= 2 {
                let holders: Vec<String> = r.approver_holders(team, &[it.name.as_str(), "k3s.cluster"]).into_iter().collect();
                v["approver_role_holders"] = json!(holders);
            }
        }
    }
    Some(v)
}

pub fn list(env: &Env, c: &Caller) -> R {
    need_member(c)?;
    let store = Store::new(env.dir);
    let cfg = store.load().map_err(|m| e(503, "unavailable", &m))?;
    let entries = cfg.entries();
    let items: Vec<Value> = entries.iter().filter_map(|en| describe_entry(env, c, &cfg, &entries, en)).collect();
    let mut out = json!({"items": items, "version": cfg.version, "file_hash": cfg.file_hash, "file_error": cfg.file_error.is_some(), "vault": if env.vault.state(&SecretRef { ns: "x".into(), name: "x".into() }) == SecretState::Locked { "locked" } else { "unlocked" }});
    if c.is_admin() {
        out["registry"] = schema::describe();
        out["safety_rules"] = safety::rules_table();
    }
    Ok(out)
}

/// Where a path points: (team, name).
fn locate(spec: &KindSpec, params: &[String]) -> (Option<String>, String) {
    match (spec.card, params.len()) {
        (_, 0) => (None, DEFAULT_NAME.into()),
        (schema::Card::Single, 1) => (Some(params[0].clone()), DEFAULT_NAME.into()),
        (_, 1) => (None, params[0].clone()),
        _ => (Some(params[0].clone()), params[1].clone()),
    }
}

fn spec_of(kind: &str) -> Result<&'static KindSpec, ApiErr> {
    kind_from_url(kind).ok_or_else(|| {
        if schema::PLANNED.contains(&kind.replacen('-', ".", 1).as_str()) {
            e(404, "not_found", "this kind is planned and not available yet")
        } else {
            e(404, "not_found", "no such kind")
        }
    })
}

/// `GET .../{kind}`: every entry of the kind the caller may see (a single value is one entry).
pub fn get(env: &Env, c: &Caller, kind: &str, params: &[String]) -> R {
    need_member(c)?;
    let spec = spec_of(kind)?;
    let store = Store::new(env.dir);
    let cfg = store.load().map_err(|m| e(503, "unavailable", &m))?;
    let entries = cfg.entries();
    let (ns, name) = locate(spec, params);
    let all = params.is_empty() && spec.card == schema::Card::Collection;
    let found: Vec<Value> = entries
        .iter()
        .filter(|en| en.item.kind == spec.id && (all || (en.item.namespace == ns && en.item.name == name)))
        .filter_map(|en| describe_entry(env, c, &cfg, &entries, en))
        .collect();
    if all {
        return Ok(json!({"kind": spec.id, "items": found}));
    }
    found.into_iter().next().ok_or_else(|| e(404, "not_found", "not configured")) // also for a team the caller is not in
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    values: BTreeMap<String, Value>,
    #[serde(default)]
    secrets: BTreeMap<String, String>,
    #[serde(default)]
    namespace: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// Codes of the `warn` rules the admin has read and accepts; required for each one on enabling.
    #[serde(default)]
    ack_warnings: Vec<String>,
}

fn parse_body(b: Value) -> Result<Body, ApiErr> {
    serde_json::from_value(b).map_err(|_| e(400, "invalid_input", "send {\"enabled\": bool, \"values\": {...}, \"secrets\": {field: value}, \"ack_warnings\": [codes]}"))
}

fn problems_text(p: &[Problem]) -> String {
    p.iter().map(|x| format!("{}: {}", x.field, x.message)).collect::<Vec<_>>().join("; ")
}

/// Build the item a PUT or a dry run means: the markers a read returned keep the stored reference, secrets get their reference.
fn build(spec: &KindSpec, ns: Option<String>, name: String, b: &Body, existing: Option<&Item>) -> Result<Item, ApiErr> {
    let mut it = Item { kind: spec.id.into(), name, namespace: ns, enabled: b.enabled.unwrap_or(false), managed: false, values: b.values.clone() };
    for (k, v) in it.values.clone() {
        if matches!(v.as_str(), Some(SET) | Some(NOT_SET) | Some(LOCKED)) {
            match existing.and_then(|x| x.values.get(&k)) {
                Some(old) => it.values.insert(k, old.clone()),
                None => it.values.remove(&k),
            };
        }
    }
    let fields = schema::secret_fields(spec);
    for (field, val) in &b.secrets {
        if !fields.contains(&field.as_str()) {
            return Err(e(400, "invalid_input", "that setting does not hold a secret"));
        }
        if val.is_empty() || val.len() > 8192 || val.chars().any(|c| c.is_control()) {
            return Err(e(400, "invalid_input", "a secret is 1 to 8192 characters without control characters"));
        }
        if !it.values.contains_key(field) {
            let r = existing.and_then(|x| x.values.get(field)).cloned().unwrap_or_else(|| json!(default_ref(spec, &it, field)));
            it.values.insert(field.clone(), r);
        }
    }
    Ok(it)
}

fn changed_fields(before: Option<&Item>, after: Option<&Item>) -> String {
    let mut names: Vec<String> = Vec::new();
    let empty = BTreeMap::new();
    let (b, a) = (before.map_or(&empty, |x| &x.values), after.map_or(&empty, |x| &x.values));
    for k in b.keys().chain(a.keys()) {
        if b.get(k) != a.get(k) && !names.contains(k) {
            names.push(k.clone());
        }
    }
    if before.map(|x| x.enabled) != after.map(|x| x.enabled) {
        names.push("enabled".into());
    }
    names.join(",")
}

fn authorize_write(c: &Caller, spec: &KindSpec, ns: Option<&str>, cfg: &Config, name: &str) -> Result<(), ApiErr> {
    need_member(c)?;
    if let Some(n) = ns {
        if !c.sees(Some(n)) {
            return Err(e(404, "not_found", "no such team")); // as for a team that does not exist
        }
    }
    if !c.writes(spec, ns) {
        return Err(e(403, "denied", "your role does not allow changing this"));
    }
    if cfg.is_managed(spec.id, ns, name) {
        return Err(e(409, "denied", "this item is managed by the operator's file and cannot be changed here"));
    }
    Ok(())
}

/// The provider an entry stands for, as the tenant's allow-list names it.
fn provider_of(item: &Item) -> String {
    match item.kind.as_str() {
        "agent.claude" => "claude".into(),
        "agent.copilot" => "copilot".into(),
        "integration.apikey" | "models.local" => item.values.get("provider").or_else(|| item.values.get("server")).and_then(|v| v.as_str()).map_or(item.name.clone(), String::from),
        _ => item.name.clone(),
    }
}

/// The tenant's rules for what a team may integrate. `Ok(Some(reply))`: the change waits for a tenant admin.
fn team_policy(env: &Env, c: &Caller, spec: &KindSpec, item: &Item, is_new: bool, cfg: &Config) -> Result<Option<Value>, ApiErr> {
    let Some(team) = item.namespace.as_deref() else { return Ok(None) };
    if !TEAM_KINDS.contains(&spec.id) {
        return Ok(None);
    }
    let pol = super::enforce::integration_policy(cfg);
    if !pol.allow_team {
        return Err(e(403, "denied", "the tenant's policy does not allow team integrations"));
    }
    let prov = provider_of(item);
    if !pol.providers.iter().any(|p| super::resolve::covers(p, &prov)) {
        return Err(e(403, "denied", "the tenant's policy does not allow this provider"));
    }
    if let Some(a) = item.values.get("allowed_models").and_then(|v| v.as_array()) {
        if a.iter().filter_map(|x| x.as_str()).any(|m| !pol.models.iter().any(|p| super::resolve::covers(p, m))) {
            return Err(e(403, "denied", "a model is not allowed by the tenant's policy"));
        }
    }
    if pol.needs_approval && is_new && item.enabled && !c.is_admin() {
        let now = super::now_secs();
        let roles = c.roles.ok_or_else(|| e(403, "denied", "no access"))?;
        // a tenant-level decision: it goes to a tenant admin, not to the team's admin
        if super::approvals::take_approval(env, spec.id, team, &item.name, c.user, now).is_none() {
            let p = super::approvals::request(env, roles, spec.id, team, &item.name, c.user, true, false, now).map_err(|m| e(400, "invalid_input", &m))?;
            return Ok(Some(json!({"ok": false, "pending_approval": p.id, "approver": "tenant_admin", "message": "a tenant admin must approve a new provider; ask again after they have"})));
        }
    }
    Ok(None)
}

fn check_safety(w: &[Warning], enabled: bool, ack: &[String]) -> Result<(), ApiErr> {
    let blocks: Vec<String> = w.iter().filter(|x| x.severity == "block").map(|x| format!("{} ({}): {}", x.code, x.field, x.text)).collect();
    if !blocks.is_empty() {
        return Err(e(400, "invalid_input", &format!("blocked by safety rules: {}", blocks.join("; "))));
    }
    if enabled {
        let need = safety::unacknowledged(w, ack);
        if !need.is_empty() {
            return Err(e(400, "ack_required", &format!("acknowledge these warnings to enable (ack_warnings): {}", need.join(", "))));
        }
    }
    Ok(())
}

fn warn_json(w: &[Warning]) -> Vec<Value> {
    w.iter().map(|x| x.json()).collect()
}

pub fn put(env: &Env, c: &Caller, kind: &str, params: &[String], body: Value) -> R {
    let spec = spec_of(kind)?;
    let b = parse_body(body)?;
    let (ns, name) = locate(spec, params);
    let store = Store::new(env.dir);
    let cfg = store.load().map_err(|m| e(503, "unavailable", &m))?;
    authorize_write(c, spec, ns.as_deref(), &cfg, &name)?;
    let before = cfg.local.iter().find(|i| i.key() == (spec.id.to_string(), ns.clone(), name.clone())).cloned();
    let item = build(spec, ns.clone(), name.clone(), &b, before.as_ref())?;
    let problems = schema::validate(&item, &cfg.all_items());
    if !problems.is_empty() {
        return Err(e(400, "invalid_input", &problems_text(&problems)));
    }
    if let Some(reply) = team_policy(env, c, spec, &item, cfg.find(spec.id, ns.as_deref(), &name).is_none(), &cfg)? {
        return Ok(reply);
    }
    let now = super::now_secs();
    let warns = warnings_for(env, &cfg, spec, &item, c.roles, now);
    check_safety(&warns, item.enabled, &b.ack_warnings)?;
    let mut key_warnings = Vec::new();
    for (field, val) in &b.secrets {
        let r = item.values.get(field).and_then(|v| v.as_str()).and_then(SecretRef::parse).ok_or_else(|| e(400, "invalid_input", "no vault reference for that secret"))?;
        if let Some(w) = catalog::key_warning(item.values.get("provider").and_then(|v| v.as_str()).unwrap_or(""), val) {
            key_warnings.push(w);
        }
        env.vault.put(&r, val).map_err(|m| e(409, "unavailable", &m))?;
    }
    let (b_hash, key) = (before.as_ref().map_or("none".to_string(), |x| x.hash()), item.key());
    let after = item.clone();
    let (version, _) = store
        .commit(c.user, &format!("put {}", spec.id), |items, cfg| {
            if cfg.is_managed(&key.0, key.1.as_deref(), &key.2) {
                return Err("this item is managed by the operator's file".into());
            }
            items.retain(|i| i.key() != key);
            items.push(item);
            Ok(())
        })
        .map_err(|m| e(409, "io", &m))?;
    if !b.secrets.is_empty() {
        store.update_meta(&meta_key(spec.id, ns.as_deref(), &name), |m| m.last_rotated = Some(now));
    }
    let secrets: Vec<&str> = b.secrets.keys().map(|s| s.as_str()).collect();
    let codes: Vec<&str> = warns.iter().filter(|w| w.severity != "info").map(|w| w.code).collect();
    let acked: Vec<&str> = b.ack_warnings.iter().map(|s| s.as_str()).filter(|a| codes.contains(a)).collect();
    let detail = format!("kind={} ns={} name={} before={} after={} fields={} secrets_set={} warnings={} acked={}", spec.id, ns.as_deref().unwrap_or("-"), name, b_hash, after.hash(), changed_fields(before.as_ref(), Some(&after)), secrets.join(","), codes.join(","), acked.join(","));
    emit(env, c.user, "tenant.config.put", &detail);
    let cfg2 = store.load().map_err(|m| e(503, "unavailable", &m))?;
    let entries = cfg2.entries();
    let state = entries.iter().find(|x| x.item.key() == after.key()).and_then(|x| describe_entry(env, c, &cfg2, &entries, x)).map(|v| v["state"].clone()).unwrap_or(Value::Null);
    Ok(json!({"ok": true, "version": version, "state": state, "warnings": warn_json(&warns), "key_warnings": key_warnings}))
}

pub fn delete(env: &Env, c: &Caller, kind: &str, params: &[String]) -> R {
    let spec = spec_of(kind)?;
    let (ns, name) = locate(spec, params);
    let store = Store::new(env.dir);
    let cfg = store.load().map_err(|m| e(503, "unavailable", &m))?;
    authorize_write(c, spec, ns.as_deref(), &cfg, &name)?;
    let key = (spec.id.to_string(), ns.clone(), name.clone());
    let before = cfg.local.iter().find(|i| i.key() == key).cloned().ok_or_else(|| e(404, "not_found", "not configured here"))?;
    let (version, _) = store
        .commit(c.user, &format!("delete {}", spec.id), |items, _| {
            items.retain(|i| i.key() != key);
            Ok(())
        })
        .map_err(|m| e(409, "io", &m))?;
    emit(env, c.user, "tenant.config.delete", &format!("kind={} ns={} name={} before={} after=none", spec.id, ns.as_deref().unwrap_or("-"), name, before.hash()));
    Ok(json!({"ok": true, "version": version}))
}

/// Dry run: the same checks as a PUT, nothing written (no secret stored, no version made).
pub fn validate(env: &Env, c: &Caller, kind: &str, body: Value) -> R {
    let spec = spec_of(kind)?;
    let b = parse_body(body)?;
    let (ns, name) = (b.namespace.clone(), b.name.clone().unwrap_or_else(|| DEFAULT_NAME.into()));
    let store = Store::new(env.dir);
    let cfg = store.load().map_err(|m| e(503, "unavailable", &m))?;
    authorize_write(c, spec, ns.as_deref(), &cfg, &name)?;
    let existing = cfg.local.iter().find(|i| i.key() == (spec.id.to_string(), ns.clone(), name.clone())).cloned();
    let item = build(spec, ns, name, &b, existing.as_ref())?;
    let problems = schema::validate(&item, &cfg.all_items());
    let mut would = Value::Null;
    let mut warns = Vec::new();
    if problems.is_empty() {
        // the state it would have, as if its secrets were already stored
        let mut entries = cfg.entries();
        entries.retain(|x| x.item.key() != item.key());
        entries.push(Entry { item: item.clone(), source: Source::Api, shadows_local: false });
        let eff = resolve(spec, &cfg.layers(&entries, spec.id, item.namespace.as_deref(), &item.name));
        let pending: Vec<&String> = b.secrets.keys().collect();
        let (st, reasons) = if pending.is_empty() { state_of(spec, &eff, env.vault.as_ref()) } else { (if eff.enabled { State::Ready } else { State::Disabled }, vec!["secrets would be stored".to_string()]) };
        would = json!({"state": st.name(), "reasons": reasons});
        warns = warnings_for(env, &cfg, spec, &item, c.roles, super::now_secs());
    }
    let blocked = warns.iter().any(|w| w.severity == "block");
    let need = safety::unacknowledged(&warns, &b.ack_warnings);
    Ok(json!({"valid": problems.is_empty() && !blocked, "problems": problems.iter().map(|p| json!({"field": p.field, "message": p.message})).collect::<Vec<_>>(), "would_be": would,
        "secrets_to_store": b.secrets.keys().collect::<Vec<_>>(), "warnings": warn_json(&warns), "ack_needed": if item.enabled { need } else { vec![] },
        "key_warnings": b.secrets.iter().filter_map(|(_, v)| catalog::key_warning(item.values.get("provider").and_then(|p| p.as_str()).unwrap_or(""), v)).collect::<Vec<_>>()}))
}

/// Admin-triggered "test connection": one cheap authenticated call to the provider's model-list endpoint, at most once a minute per
/// entry. Stores ok/failed, the time and the HTTP status; never the response body and never the key.
pub fn test(env: &Env, c: &Caller, kind: &str, params: &[String], now: u64) -> R {
    probe(env, c, kind, params, now, false)
}

/// Model discovery for the same entries and the same checks as [`test`] (ready, not blocked, admins only): asks the host for its model list
/// and returns the parsed names (`{"models": [...]}`) so the admin can fill `allowed_models`. Stores nothing and writes one audit line.
pub fn discover(env: &Env, c: &Caller, kind: &str, params: &[String], now: u64) -> R {
    probe(env, c, kind, params, now, true)
}

fn probe(env: &Env, c: &Caller, kind: &str, params: &[String], now: u64, discover: bool) -> R {
    let spec = spec_of(kind)?;
    if !matches!(spec.id, "integration.apikey" | "models.local") {
        return Err(e(400, "invalid_input", "only API-key integrations and local model hosts can be tested"));
    }
    need_member(c)?;
    let (ns, name) = locate(spec, params);
    if let Some(n) = ns.as_deref() {
        if !c.sees(Some(n)) {
            return Err(e(404, "not_found", "no such team"));
        }
    }
    if !c.writes(spec, ns.as_deref()) {
        return Err(e(403, "denied", "your role does not allow this"));
    }
    let store = Store::new(env.dir);
    let cfg = store.load().map_err(|m| e(503, "unavailable", &m))?;
    let entries = cfg.entries();
    let en = entries.iter().find(|x| x.item.kind == spec.id && x.item.namespace == ns && x.item.name == name).ok_or_else(|| e(404, "not_found", "not configured"))?;
    let eff = resolve(spec, &cfg.layers(&entries, spec.id, ns.as_deref(), &name));
    let warns = safety::evaluate(&safety::Ctx { spec, item: &en.item, eff: &eff, cfg: &cfg, roles: c.roles, meta: &store.meta(), now });
    if let Some(w) = warns.iter().find(|w| w.severity == "block") {
        return Err(e(409, "blocked", &format!("blocked by a safety rule ({}): {}", w.code, w.text)));
    }
    let (st, reasons) = state_of(spec, &eff, env.vault.as_ref());
    if st != State::Ready {
        return Err(e(409, "unavailable", &format!("the entry is {} ({})", st.name(), reasons.join("; "))));
    }
    let mk = meta_key(spec.id, ns.as_deref(), &name);
    if let Some((at, _, _)) = store.meta().get(&mk).and_then(|m| m.test).filter(|_| !discover) {
        if now.saturating_sub(at) < 60 {
            return Err(e(429, "rate_limited", "a connection test runs at most once a minute per entry"));
        }
    }
    let (url, auth, key_field) = if spec.id == "models.local" {
        let srv = eff.values.get("server").and_then(|v| v.as_str()).and_then(catalog::local_server);
        let path = srv.map_or("/v1/models", |s| if eff.values.get("api_style").and_then(|v| v.as_str()) == Some("ollama") { "/api/tags" } else { s.list_path });
        let host = eff.values.get("host").and_then(|v| v.as_str()).unwrap_or("");
        (format!("{}://{host}:{}{path}", eff.values.get("scheme").and_then(|v| v.as_str()).unwrap_or("http"), eff.values.get("port").and_then(|v| v.as_u64()).unwrap_or(0)), "bearer", "token")
    } else {
        let pid = eff.values.get("provider").and_then(|v| v.as_str()).unwrap_or("other");
        let p = catalog::provider(pid);
        (format!("{}{}", catalog::base_url(pid, &eff.values).unwrap_or_default(), p.map_or("", |p| p.list_path)), p.map_or("bearer", |p| p.auth), "api_key")
    };
    let tester = catalog::tester_for(env.dir);
    let style = if spec.id == "models.local" { eff.values.get("api_style").and_then(|v| v.as_str()).unwrap_or("openai_compatible").to_string() } else if eff.values.get("provider").and_then(|v| v.as_str()) == Some("gemini") { "gemini".into() } else { "openai_compatible".to_string() };
    let mut run = |key: &str| -> Result<String, String> {
        if discover {
            return tester.list_models(&catalog::TestRequest { url: &url, auth, key }, &style).map(|m| m.join("\n"));
        }
        let r = tester.test(&catalog::TestRequest { url: &url, auth, key })?;
        Ok(format!("{}:{}", r.ok, r.status))
    };
    let res = match eff.values.get(key_field).and_then(|v| v.as_str()).and_then(SecretRef::parse) {
        Some(r) => env.vault.use_secret(&r, "test-connection", &mut run),
        None => run(""),
    };
    let out = res.map_err(|m| e(503, "unavailable", &m))?;
    if discover {
        let models: Vec<&str> = out.lines().collect();
        emit(env, c.user, "tenant.integration.discover", &format!("kind={} team={} name={} models={}", spec.id, ns.as_deref().unwrap_or("-"), name, models.len()));
        return Ok(json!({"models": models}));
    }
    let (ok, status) = out.split_once(':').map_or((false, 0), |(a, b)| (a == "true", b.parse::<u16>().unwrap_or(0)));
    store.update_meta(&mk, |m| m.test = Some((now, ok, status)));
    emit(env, c.user, "tenant.integration.test", &format!("kind={} team={} name={} ok={ok} status={status}", spec.id, ns.as_deref().unwrap_or("-"), name));
    Ok(json!({"ok": ok, "status": status, "at": now}))
}

pub fn versions(env: &Env, c: &Caller) -> R {
    need_admin(c)?;
    let store = Store::new(env.dir);
    let cfg = store.load().map_err(|m| e(503, "unavailable", &m))?;
    Ok(json!({"current": cfg.version, "versions": store.versions()}))
}

pub fn rollback(env: &Env, c: &Caller, body: Value) -> R {
    need_admin(c)?;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct B {
        version: u64,
    }
    let b: B = serde_json::from_value(body).map_err(|_| e(400, "invalid_input", "send {\"version\": n}"))?;
    let store = Store::new(env.dir);
    let doc = store.version_doc(b.version).map_err(|m| e(404, "not_found", &m))?;
    let bad: Vec<String> = doc.items.iter().flat_map(|i| schema::validate(i, &doc.items).into_iter().map(|p| format!("{}: {}", i.kind, p.message))).collect();
    if !bad.is_empty() {
        return Err(e(400, "invalid_input", &bad.join("; ")));
    }
    let before = store.load().map_err(|m| e(503, "unavailable", &m))?;
    let (new, hash) = store
        .commit(c.user, &format!("rollback to {}", b.version), |items, _| {
            *items = doc.items.iter().cloned().map(|mut i| {
                i.managed = false;
                i
            }).collect();
            Ok(())
        })
        .map_err(|m| e(409, "io", &m))?;
    emit(env, c.user, "tenant.config.rollback", &format!("to={} new_version={} from_version={} after={}", b.version, new, before.version, hash));
    Ok(json!({"ok": true, "version": new}))
}

/// The tenant's configuration without a single secret: references, not values. Same schema the operator's file uses.
pub fn export(env: &Env, c: &Caller) -> R {
    need_admin(c)?;
    let cfg = Store::new(env.dir).load().map_err(|m| e(503, "unavailable", &m))?;
    let items: Vec<Item> = cfg.all_items().into_iter().map(|mut i| {
        i.managed = false;
        i
    }).collect();
    emit(env, c.user, "tenant.config.export", &format!("items={}", items.len()));
    Ok(json!({"schema": SCHEMA_VERSION, "items": items}))
}

/// Import a document (the shape of [`export`]). All or nothing; items the operator's file manages are skipped and listed.
pub fn import(env: &Env, c: &Caller, body: Value) -> R {
    need_admin(c)?;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct B {
        doc: Doc,
        #[serde(default)]
        dry_run: bool,
        #[serde(default)]
        replace: bool,
    }
    let b: B = serde_json::from_value(body).map_err(|_| e(400, "invalid_input", "send {\"doc\": {\"items\": [...]}, \"dry_run\": bool, \"replace\": bool}"))?;
    let store = Store::new(env.dir);
    let cfg = store.load().map_err(|m| e(503, "unavailable", &m))?;
    let mut keep: Vec<Item> = Vec::new();
    let mut skipped = Vec::new();
    for mut i in b.doc.items {
        i.managed = false;
        if cfg.is_managed(&i.kind, i.namespace.as_deref(), &i.name) {
            skipped.push(format!("{}/{}", i.kind, i.name));
        } else {
            keep.push(i);
        }
    }
    let mut problems = Vec::new();
    for i in &keep {
        for p in schema::validate(i, &keep) {
            problems.push(format!("{}/{}: {}: {}", i.kind, i.name, p.field, p.message));
        }
    }
    if !problems.is_empty() {
        return Err(e(400, "invalid_input", &problems.join("; ")));
    }
    if b.dry_run {
        return Ok(json!({"valid": true, "would_import": keep.len(), "skipped_managed": skipped}));
    }
    let n = keep.len();
    let (version, hash) = store
        .commit(c.user, "import", |items, _| {
            if b.replace {
                items.clear();
            }
            for i in keep {
                items.retain(|x| x.key() != i.key());
                items.push(i);
            }
            Ok(())
        })
        .map_err(|m| e(409, "io", &m))?;
    emit(env, c.user, "tenant.config.import", &format!("items={n} replace={} version={version} after={hash}", b.replace));
    Ok(json!({"ok": true, "version": version, "imported": n, "skipped_managed": skipped}))
}
