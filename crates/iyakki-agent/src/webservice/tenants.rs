// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! Which tenants a signed-in person may use, and the switch between them (`GET /api/v1/tenants`, `POST /api/v1/tenant/switch`).
//!
//! Plan: `docs/development/CLOUD-MULTITENANT-PLAN.md` (3.3 the switcher, 3.4 the two ways in). A service process is started for one
//! **home** tenant (its volume is `data_dir`). The operator may list more tenants in `<data_dir>/cloud/tenants.json`:
//!
//! ```json
//! {"home": {"id": "acme", "name": "Acme Ltd"},
//!  "tenants": [{"id": "beta", "name": "Beta GmbH", "host": "beta.example.org", "data_dir": "/var/lib/iyakki-beta"}]}
//! ```
//!
//! Who belongs to a tenant is never copied into that file: it is the tenant's own `roles.json` (`cloudroles.rs`) on its volume, so a
//! removal there ends the right here at once. A person who is in no list of a tenant gets the answer for a tenant that is not there,
//! never "no access". A switch binds the session's token to the target (the next request works on the target's volume), stops the
//! runs the person had going, and tells the machine to lock what it kept unlocked for the previous tenant.
//!
//! The directory names only tenants a process can reach on its own volumes; a tenant on another host is named with its `host`, and
//! the answer carries its address so the page opens it there (a different pod holds a different session: the person signs in again
//! at that address, which no token or password ever crosses).

use super::handlers::{Ctx, Out};
use super::server::{json_body, Fail, Shared};
use crate::cloudroles;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The one tenant of a machine that is not a cloud pod (the embedded service of the desktop's browser server).
pub const THIS_MACHINE: (&str, &str) = ("this-machine", "This machine");
/// The home tenant of a pod whose operator named none.
const HOME_DEFAULT: (&str, &str) = ("default", "This tenant");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub host: Option<String>,
    /// The tenant's volume (`None` for the home tenant: the service's own `data_dir`).
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Directory {
    pub home: Entry,
    pub others: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HomeFile {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryFile {
    id: String,
    name: String,
    #[serde(default)]
    host: Option<String>,
    data_dir: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirFile {
    #[serde(default)]
    home: Option<HomeFile>,
    #[serde(default)]
    tenants: Vec<EntryFile>,
}

pub fn file(data_dir: &Path) -> PathBuf {
    data_dir.join("cloud").join("tenants.json")
}

pub fn valid_id(s: &str) -> bool {
    crate::cloudtenant::dns_label(s)
}

pub(crate) fn valid_name(s: &str) -> bool {
    let n = s.chars().count();
    (1..=80).contains(&n) && !s.chars().any(|c| c.is_control())
}

fn home_entry(h: Option<HomeFile>) -> Entry {
    match h {
        Some(h) if valid_id(&h.id) && valid_name(&h.name) => Entry { id: h.id, name: h.name, host: None, data_dir: None },
        _ => Entry { id: HOME_DEFAULT.0.into(), name: HOME_DEFAULT.1.into(), host: None, data_dir: None },
    }
}

/// The directory of a pod. A missing or unreadable file leaves only the home tenant (the safe reading); bad entries are skipped.
pub fn load(data_dir: &Path) -> Directory {
    let parsed = match std::fs::read_to_string(file(data_dir)) {
        Ok(t) => serde_json::from_str::<DirFile>(&t).map_err(|e| e.to_string()).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    };
    let f = match parsed {
        Ok(Some(f)) => f,
        Ok(None) => return Directory { home: home_entry(None), others: vec![] },
        Err(e) => {
            log::warn!("service: the tenant list is unreadable ({e}); only the home tenant is offered");
            return Directory { home: home_entry(None), others: vec![] };
        }
    };
    let home = home_entry(f.home);
    let mut others: Vec<Entry> = Vec::new();
    for t in f.tenants {
        let ok = valid_id(&t.id) && valid_name(&t.name) && t.id != home.id && !others.iter().any(|o| o.id == t.id) && t.data_dir.starts_with('/') && !t.data_dir.contains("..") && t.host.as_deref().is_none_or(crate::cloudtenant::host_name);
        if ok {
            others.push(Entry { id: t.id, name: t.name, host: t.host, data_dir: Some(PathBuf::from(t.data_dir)) });
        } else {
            log::warn!("service: a tenant entry was skipped (bad id, name, host or folder, or a repeat)");
        }
    }
    Directory { home, others }
}

impl Directory {
    pub fn find(&self, id: &str) -> Option<&Entry> {
        if self.home.id == id {
            return Some(&self.home);
        }
        self.others.iter().find(|e| e.id == id)
    }
    fn all(&self) -> impl Iterator<Item = &Entry> {
        std::iter::once(&self.home).chain(self.others.iter())
    }
}

/// The person's role in a tenant, lower case, or `None` when they are not in it. The tenant's own lists decide. A tenant with no
/// lists yet admits everybody only when it is the home tenant (as `tenant_gate` does for its personal routes).
fn role_in(e: &Entry, home_dir: &Path, user: &str) -> Option<String> {
    let dir = e.data_dir.as_deref().unwrap_or(home_dir);
    match cloudroles::load(&cloudroles::path(dir)) {
        Ok(Some(r)) => r.tenant_role(user).map(|t| format!("{t:?}").to_lowercase()),
        Ok(None) if e.data_dir.is_none() => Some("member".into()),
        _ => None,
    }
}

/// The folder the signed-in request works in: the home volume, or the volume of the tenant its session was switched to. A switch to a
/// tenant the person has since been removed from falls back to home (where the usual checks apply), never to a volume they lost.
pub(super) fn data_dir_for(sh: &Shared, token: Option<&str>, user: &str) -> PathBuf {
    let home = PathBuf::from(&sh.cfg.data_dir);
    let Some(id) = token.and_then(|t| sh.tokens.tenant_of(t)) else { return home };
    let dir = load(&home);
    match dir.find(&id) {
        Some(e) if role_in(e, &home, user).is_some() => e.data_dir.clone().unwrap_or(home),
        _ => home,
    }
}

fn current_id(sh: &Shared, token: Option<&str>, user: &str, dir: &Directory, home: &Path) -> String {
    match token.and_then(|t| sh.tokens.tenant_of(t)).and_then(|id| dir.find(&id).filter(|e| role_in(e, home, user).is_some()).map(|e| e.id.clone())) {
        Some(id) => id,
        None => dir.home.id.clone(),
    }
}

fn entry_json(e: &Entry, role: Option<&str>, current: bool) -> Value {
    json!({"id": e.id, "name": e.name, "role": role, "current": current})
}

/// `GET /api/v1/tenants`: the tenants this person may use. `?scope=all` (a service administrator only) lists every tenant of the
/// directory, with the person's own role where they have one.
pub fn list(c: &Ctx) -> Out {
    let me = c.who();
    if !c.sh.profile.is_cloud() {
        // a machine that is not a pod: one tenant, "this machine", nothing to switch to
        return Ok((200, json!({"tenants": [entry_json(&Entry { id: THIS_MACHINE.0.into(), name: THIS_MACHINE.1.into(), host: None, data_dir: None }, Some(&me.role), true)], "current": THIS_MACHINE.0, "can_switch": false, "scope": "own"}), String::new()));
    }
    let all = match c.req.query.get("scope").map(String::as_str) {
        None | Some("") | Some("own") => false,
        Some("all") => {
            if !me.admin {
                return Err(Fail::new(403, "denied", "listing every tenant is for the service's administrators"));
            }
            true
        }
        Some(_) => return Err(Fail::new(400, "invalid_input", "scope is own or all")),
    };
    let home = PathBuf::from(&c.sh.cfg.data_dir);
    let dir = load(&home);
    let cur = current_id(c.sh, c.token, &me.user, &dir, &home);
    let mut out = Vec::new();
    for e in dir.all() {
        let role = role_in(e, &home, &me.user);
        if role.is_some() || all {
            out.push(entry_json(e, role.as_deref(), e.id == cur));
        }
    }
    if all {
        c.sh.deps.audit.record(&me.user, "api.tenants.all", &format!("count={}", out.len()));
    }
    let can_switch = out.iter().filter(|t| !t["role"].is_null()).count() > 1;
    Ok((200, json!({"tenants": out, "current": cur, "can_switch": can_switch, "scope": if all { "all" } else { "own" }}), String::new()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SwitchReq {
    tenant: String,
}

/// `POST /api/v1/tenant/switch {"tenant": ID}`.
pub fn switch(c: &Ctx) -> Out {
    let me = c.who();
    let r: SwitchReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"tenant\": \"<id>\"}"))?;
    let (ok, wait) = c.sh.switch_limit.allow(&me.user);
    if !ok {
        return Err(Fail::new(429, "rate_limited", &format!("too many switches; wait {wait} seconds")));
    }
    let not_there = || Fail::new(404, "not_found", "no such tenant");
    if !valid_id(&r.tenant) {
        return Err(not_there());
    }
    let home = PathBuf::from(&c.sh.cfg.data_dir);
    let dir = load(&home);
    // a tenant the person is not in answers like one that does not exist
    let target = dir.find(&r.tenant).ok_or_else(not_there)?;
    let role = role_in(target, &home, &me.user).ok_or_else(not_there)?;
    let from = current_id(c.sh, c.token, &me.user, &dir, &home);
    let tok = c.token.ok_or_else(|| Fail::new(401, "unauthorized", "sign in first"))?;
    if from == target.id {
        return Ok((200, json!({"switched": false, "tenant": entry_json(target, Some(&role), true), "previous": from}), format!("from={from} to={} same", target.id)));
    }
    // the session first: whatever fails after this, nothing of the old tenant is left open
    let bind = if target.id == dir.home.id { None } else { Some(target.id.clone()) };
    if !c.sh.tokens.set_tenant(tok, bind) {
        return Err(Fail::new(401, "unauthorized", "sign in first"));
    }
    // work the person had going belongs to the previous tenant
    let mut stopped = 0;
    for run in c.sh.runs.list(me) {
        if run["status"] == "running" {
            if let Some(id) = run["id"].as_str() {
                if c.sh.runs.stop(me, id).is_ok() {
                    stopped += 1;
                }
            }
        }
    }
    // lock what the machine kept unlocked for the previous tenant (vault); streamed uploads and downloads hold nothing between requests
    c.sh.deps.extras.tenant_switched(me, &from, &target.id);
    let url = target.host.as_ref().filter(|h| c.req.header("host").map(|x| x.split(':').next().unwrap_or("")) != Some(h.as_str())).map(|h| format!("https://{h}/ui/"));
    Ok((200, json!({"switched": true, "tenant": entry_json(target, Some(&role), true), "previous": from, "runs_stopped": stopped, "vault_locked": true, "url": url}), format!("from={from} to={} runs_stopped={stopped}", target.id)))
}
