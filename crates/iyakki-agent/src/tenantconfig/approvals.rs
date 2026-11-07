//! One approval router for every tenant-config enforcement point (owner decision 2026-11-04: all approval goes to the admin of the
//! requester's team). Eligible approvers = admins of THAT team (namespace) except the requester. None eligible (the requester is the sole
//! admin, or the team has no admin) escalates to the tenant admins, flagged as an escalation. A tenant rule may only narrow it (a kind that
//! needs a tenant admin). An admin of another team is never eligible. Nobody approves their own request.
//!
//! Requests are kept in `<data_dir>/cloud/config/approvals.json` (names, kinds and times only) and expire (default 24 h,
//! `IYAKKI_APPROVAL_TTL_SECS`, 60 s to 30 days).

use super::api::{emit, ApiErr, Caller, Env};
use crate::cloudroles::{Roles, TenantRole};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Mutex;

pub const DEFAULT_TTL_SECS: u64 = 86_400;

/// Kinds of request that are not configuration items: a flow's `approval` node (item = `<run>_<node>`), see `cloudflows::approval`.
pub const EXTRA_KINDS: &[&str] = &["flow.approval"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub approvers: Vec<String>,
    /// The team had nobody else, so tenant admins decide.
    pub escalated: bool,
    /// The tenant's rule narrowed it to tenant admins.
    pub tenant_only: bool,
    /// `role` (holders of the approver role), `team_admin`, `escalation` or `tenant`.
    pub via: &'static str,
}

fn tenant_admins(roles: &Roles, requester: &str) -> Vec<String> {
    roles.tenant.iter().filter(|(n, r)| **r >= TenantRole::Admin && n.as_str() != requester).map(|(n, _)| n.clone()).collect()
}

/// Who may approve `requester`'s request in `team`, in this order: (1) holders of the approver role for one of `scopes` (people and
/// expanded groups, the requester excluded; with `role_only` they alone, otherwise with the team's admins), (2) the team's admins
/// except the requester, (3) the tenant admins, flagged as an escalation. A tenant rule (`tenant_only`) narrows to tenant admins.
/// An admin or holder of another team is never eligible. Nobody approves their own request.
pub fn route(roles: &Roles, team: &str, requester: &str, tenant_only: bool, scopes: &[&str], role_only: bool) -> Route {
    if tenant_only {
        return Route { approvers: tenant_admins(roles, requester), escalated: false, tenant_only: true, via: "tenant" };
    }
    let mut holders = roles.approver_holders(team, scopes);
    holders.remove(requester);
    let admins: Vec<String> = roles
        .namespaces
        .get(team)
        .into_iter()
        .flatten()
        .filter(|(n, r)| **r == crate::cloudroles::NsRole::Admin && n.as_str() != requester && roles.tenant_role(n).is_some())
        .map(|(n, _)| n.clone())
        .collect();
    if !holders.is_empty() {
        let mut a: Vec<String> = holders.into_iter().collect();
        if !role_only {
            for x in admins {
                if !a.contains(&x) {
                    a.push(x);
                }
            }
        }
        return Route { approvers: a, escalated: false, tenant_only: false, via: "role" };
    }
    if !admins.is_empty() {
        return Route { approvers: admins, escalated: false, tenant_only: false, via: "team_admin" };
    }
    Route { approvers: tenant_admins(roles, requester), escalated: true, tenant_only: false, via: "escalation" }
}

pub fn ttl_secs() -> u64 {
    std::env::var("IYAKKI_APPROVAL_TTL_SECS").ok().and_then(|v| v.parse::<u64>().ok()).map_or(DEFAULT_TTL_SECS, |v| v.clamp(60, 30 * 86_400))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub id: String,
    pub kind: String,
    pub team: String,
    pub item: String,
    pub requester: String,
    pub created: u64,
    pub expires: u64,
    pub tenant_only: bool,
    /// Only holders of the approver role (not the team's admins) decide, while any holder exists.
    #[serde(default)]
    pub role_only: bool,
    /// pending, approved, denied or used
    pub state: String,
    #[serde(default)]
    pub decided_by: Option<String>,
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

fn file(env: &Env) -> std::path::PathBuf {
    env.dir.join("cloud").join("config").join("approvals.json")
}

/// Every request on file (pending, decided, expired within the week), read-only, for the notification hub's reminders. No decision is
/// possible through it.
pub fn snapshot(dir: &std::path::Path) -> Vec<Pending> {
    std::fs::read_to_string(dir.join("cloud").join("config").join("approvals.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn load(env: &Env) -> Vec<Pending> {
    std::fs::read_to_string(file(env)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(env: &Env, mut v: Vec<Pending>, now: u64) -> Result<(), String> {
    v.retain(|p| p.expires + 7 * 86_400 > now); // history of a week after expiry
    if v.len() > 2000 {
        let cut = v.len() - 2000;
        v.drain(..cut);
    }
    iyakki_schema::write_atomic(&file(env), serde_json::to_string_pretty(&v).unwrap_or_default().as_bytes(), 0o600)
}

fn live(p: &Pending, now: u64) -> bool {
    p.state == "pending" && p.expires > now
}

/// A request for approval, from an enforcement point that answered "needs approval".
pub fn request(env: &Env, roles: &Roles, kind: &str, team: &str, item: &str, requester: &str, tenant_only: bool, role_only: bool, now: u64) -> Result<Pending, String> {
    if (super::schema::kind(kind).is_none() && !EXTRA_KINDS.contains(&kind)) || !crate::cloudstorage::plain_name(team) || !crate::cloudstorage::plain_name(item) {
        return Err("not a kind, team or item name".into());
    }
    let _g = lock();
    let mut all = load(env);
    if let Some(p) = all.iter().find(|p| live(p, now) && p.kind == kind && p.team == team && p.item == item && p.requester == requester) {
        return Ok(p.clone());
    }
    let r = route(roles, team, requester, tenant_only, &[item, kind], role_only);
    let id = crate::cloudkeys::random::<8>().map(|b| crate::cloudkeys::hex(&b))?;
    let p = Pending { id, kind: kind.into(), team: team.into(), item: item.into(), requester: requester.into(), created: now, expires: now + ttl_secs(), tenant_only, role_only, state: "pending".into(), decided_by: None };
    all.push(p.clone());
    save(env, all, now)?;
    emit(env, requester, "tenant.approval.request", &format!("id={} kind={kind} team={team} item={item} requester={requester} approvers={} escalated={} via={} tenant_only={tenant_only} role_only={role_only}", p.id, r.approvers.join(","), r.escalated, r.via));
    Ok(p)
}

fn eligible(roles: &Roles, p: &Pending, who: &str) -> bool {
    who != p.requester && route(roles, &p.team, &p.requester, p.tenant_only, &[&p.item, &p.kind], p.role_only).approvers.iter().any(|a| a == who)
}

fn view(roles: &Roles, p: &Pending) -> Value {
    let r = route(roles, &p.team, &p.requester, p.tenant_only, &[&p.item, &p.kind], p.role_only);
    json!({"id": p.id, "kind": p.kind, "team": p.team, "item": p.item, "requester": p.requester, "created": p.created, "expires": p.expires, "escalated": r.escalated, "tenant_only": p.tenant_only})
}

fn e(status: u16, code: &'static str, m: &str) -> ApiErr {
    ApiErr { status, code, message: m.into() }
}

/// `GET /api/v1/tenant/approvals`: the live requests this person may decide. Nobody else sees them.
pub fn list(env: &Env, c: &Caller, now: u64) -> Result<Value, ApiErr> {
    let roles = c.roles.ok_or_else(|| e(403, "denied", "no access"))?;
    roles.tenant_role(c.user).ok_or_else(|| e(403, "denied", "no access"))?;
    let items: Vec<Value> = load(env).iter().filter(|p| live(p, now) && eligible(roles, p, c.user)).map(|p| view(roles, p)).collect();
    Ok(json!({"approvals": items}))
}

/// `POST /api/v1/tenant/approvals/{id}/approve|deny`. A request the person may not decide is "not found".
pub fn decide(env: &Env, c: &Caller, id: &str, approve: bool, now: u64) -> Result<Value, ApiErr> {
    let roles = c.roles.ok_or_else(|| e(403, "denied", "no access"))?;
    roles.tenant_role(c.user).ok_or_else(|| e(403, "denied", "no access"))?;
    let _g = lock();
    let mut all = load(env);
    let Some(i) = all.iter().position(|p| p.id == id && eligible(roles, p, c.user)) else {
        if all.iter().any(|p| p.id == id && p.requester == c.user) {
            emit(env, c.user, "tenant.approval.refused", &format!("id={id} reason=self"));
            return Err(e(403, "denied", "an approval cannot come from the person who asked"));
        }
        return Err(e(404, "not_found", "no such request"));
    };
    if all[i].state != "pending" {
        return Err(e(409, "invalid_input", "already decided"));
    }
    if all[i].expires <= now {
        return Err(e(409, "invalid_input", "this request has expired; ask again"));
    }
    all[i].state = if approve { "approved" } else { "denied" }.into();
    all[i].decided_by = Some(c.user.into());
    let p = all[i].clone();
    save(env, all, now).map_err(|m| e(503, "io", &m))?;
    let r = route(roles, &p.team, &p.requester, p.tenant_only, &[&p.item, &p.kind], p.role_only);
    emit(env, c.user, if approve { "tenant.approval.approve" } else { "tenant.approval.deny" }, &format!("id={} kind={} team={} item={} requester={} approver={} escalated={} via={}", p.id, p.kind, p.team, p.item, p.requester, c.user, r.escalated, r.via));
    Ok(json!({"ok": true, "state": p.state}))
}

/// For the enforcement point: who approved this request, once, and still valid. Marks it used.
pub fn take_approval(env: &Env, kind: &str, team: &str, item: &str, requester: &str, now: u64) -> Option<String> {
    let _g = lock();
    let mut all = load(env);
    let p = all.iter_mut().find(|p| p.state == "approved" && p.expires > now && p.kind == kind && p.team == team && p.item == item && p.requester == requester)?;
    p.state = "used".into();
    let by = p.decided_by.clone();
    let _ = save(env, all, now);
    by
}

/// What the flow bridge needs to know about one request: (state, decided_by, created, expires). `None`: no such request.
pub fn state_of(env: &Env, id: &str) -> Option<(String, Option<String>, u64, u64)> {
    let _g = lock();
    load(env).into_iter().find(|p| p.id == id).map(|p| (p.state, p.decided_by, p.created, p.expires))
}

/// Close a request that is still pending because the thing waiting for it timed out or was cancelled: it leaves everybody's list.
pub fn close(env: &Env, id: &str, by: &str, now: u64) -> bool {
    let _g = lock();
    let mut all = load(env);
    let Some(p) = all.iter_mut().find(|p| p.id == id && p.state == "pending") else { return false };
    p.state = "denied".into();
    p.decided_by = Some(by.into());
    let line = format!("id={id} kind={} team={} item={} requester={} closed_by={by}", p.kind, p.team, p.item, p.requester);
    let who = p.requester.clone();
    let _ = save(env, all, now);
    emit(env, &who, "tenant.approval.close", &line);
    true
}

/// The approvers a request would go to now (for notifications).
pub fn approvers_of(env: &Env, roles: &Roles, id: &str) -> Vec<String> {
    let _g = lock();
    load(env).iter().find(|p| p.id == id).map(|p| route(roles, &p.team, &p.requester, p.tenant_only, &[&p.item, &p.kind], p.role_only).approvers).unwrap_or_default()
}
