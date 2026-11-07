//! `/api/v1/tenant/config...`: the web face of `tenantconfig::api` (plan: `docs/development/TENANT-CONFIG-PLAN.md`). Thin: it finds who is
//! asking and what the tenant's roles say, calls the operation, and maps its refusal to the service's error form.

use super::handlers::{Ctx, Out};
use super::server::{json_body, Fail};
use crate::cloudaccess::Access;
use crate::tenantconfig::api::{self, ApiErr, Caller, ConfigAudit, Env};
use serde_json::Value;

struct Sink<'a>(&'a dyn iyakki_core_api::AuditSink);

impl ConfigAudit for Sink<'_> {
    fn record(&self, who: &str, event: &str, detail: &str) {
        self.0.record(who, event, detail);
    }
}

fn fail(e: ApiErr) -> Fail {
    Fail::new(e.status, e.code, &e.message)
}

/// The operation of a route id: `tcfg_put_t` is `put`.
pub fn handle(c: &Ctx, id: &str) -> Out {
    if id.starts_with("tapprovals_") || id.starts_with("tteam_") {
        return handle_other(c, id);
    }
    let dir = c.data_dir();
    let roles = Access::new(&dir).roles().ok().flatten();
    let who = c.who();
    let caller = Caller { user: &who.user, roles: roles.as_ref() };
    let sink = Sink(c.sh.deps.audit.as_ref());
    let env = Env { dir: &dir, vault: crate::tenantconfig::vault::for_user(&dir, &who.user), audit: &sink };
    let op = id.strip_prefix("tcfg_").unwrap_or(id).split('_').next().unwrap_or("");
    let body = || -> Result<Value, Fail> { json_body(c.req) };
    let kind = c.params.first().map(String::as_str).unwrap_or("");
    let rest = c.params.get(1..).unwrap_or(&[]);
    let (v, detail) = match op {
        "list" => (api::list(&env, &caller), String::new()),
        "versions" => (api::versions(&env, &caller), String::new()),
        "export" => (api::export(&env, &caller), String::new()),
        "rollback" => (api::rollback(&env, &caller, body()?), String::new()),
        "import" => (api::import(&env, &caller, body()?), String::new()),
        "test" => (api::test(&env, &caller, kind, rest, crate::tenantconfig::now_secs()), format!("kind={kind}")),
        "discover" => (api::discover(&env, &caller, kind, rest, crate::tenantconfig::now_secs()), format!("kind={kind}")),
        "validate" => (api::validate(&env, &caller, kind, body()?), format!("kind={kind}")),
        "get" => (api::get(&env, &caller, kind, rest), String::new()),
        "put" => (api::put(&env, &caller, kind, rest, body()?), format!("kind={kind}")),
        "delete" => (api::delete(&env, &caller, kind, rest), format!("kind={kind}")),
        _ => return Err(Fail::new(501, "unsupported", "not implemented")),
    };
    v.map(|v| (200, v, detail.chars().filter(|ch| ch.is_ascii_alphanumeric() || "=._-".contains(*ch)).collect())).map_err(fail)
}

// ---------------------------------------------------------------------------------------------------------------- approvals, approver roles, groups

fn change_err(e: String) -> Fail {
    if e.contains("no access") || e.contains("not allowed") {
        Fail::new(403, "denied", &e)
    } else {
        Fail::new(400, "invalid_input", &e)
    }
}

/// The roles of a team the caller manages. A team the caller cannot see is "not found"; one they see and may not manage is 403.
fn manager_view(c: &Ctx, ns: &str) -> Result<crate::cloudroles::Roles, Fail> {
    let dir = c.data_dir();
    let roles = Access::new(&dir).roles().ok().flatten().ok_or_else(|| Fail::new(403, "denied", "no access"))?;
    let me = &c.who().user;
    if roles.tenant_role(me).is_none() {
        return Err(Fail::new(403, "denied", "no access"));
    }
    if ns == "_tenant" {
        return if roles.tenant_role(me).is_some_and(|t| t >= crate::cloudroles::TenantRole::Admin) { Ok(roles) } else { Err(Fail::new(403, "denied", "tenant administrators only")) };
    }
    let seen = roles.namespaces.contains_key(ns) && (roles.ns_role(ns, me).is_some() || roles.tenant_role(me).is_some_and(|t| t >= crate::cloudroles::TenantRole::Admin));
    if !seen {
        return Err(Fail::new(404, "not_found", "no such team"));
    }
    if !roles.can_manage_team(me, ns) {
        return Err(Fail::new(403, "denied", "team administrators only"));
    }
    Ok(roles)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Holders {
    #[serde(default)]
    people: Vec<String>,
    #[serde(default)]
    groups: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupBody {
    members: Vec<String>,
}

pub fn handle_other(c: &Ctx, id: &str) -> Out {
    let dir = c.data_dir();
    let who = c.who().user.clone();
    let now = crate::tenantconfig::now_secs();
    let acc = Access::new(&dir);
    let owner = |ns: &str| if ns == "_tenant" { crate::cloudroles::TENANT_WIDE.to_string() } else { ns.to_string() };
    let line = |ch: crate::cloudroles::Change| ch.0.chars().filter(|x| !x.is_control()).collect::<String>();
    if id.starts_with("tapprovals_") {
        let roles = acc.roles().ok().flatten();
        let sink = Sink(c.sh.deps.audit.as_ref());
        let env = Env { dir: &dir, vault: crate::tenantconfig::vault::for_user(&dir, &who), audit: &sink };
        let caller = Caller { user: &who, roles: roles.as_ref() };
        let r = match id {
            "tapprovals_list" => crate::tenantconfig::approvals::list(&env, &caller, now),
            "tapprovals_approve" => crate::tenantconfig::approvals::decide(&env, &caller, &c.params[0], true, now),
            _ => crate::tenantconfig::approvals::decide(&env, &caller, &c.params[0], false, now),
        };
        return r.map(|v| (200, v, String::new())).map_err(fail);
    }
    let ns = c.params.first().map(String::as_str).unwrap_or("");
    let roles = manager_view(c, ns)?;
    let ok = |v: Value, d: String| Ok((200, v, d));
    match id {
        "tteam_approvers_get" => {
            let m = roles.approvers.get(ns).cloned().unwrap_or_default();
            let out: serde_json::Map<String, Value> = m
                .iter()
                .map(|(s, h)| (s.clone(), serde_json::json!({"people": h.people, "groups": h.groups, "expanded": roles.approver_holders(ns, &[s.as_str()])})))
                .collect();
            ok(serde_json::json!({"team": ns, "approvers": out}), String::new())
        }
        "tteam_approver_grant" | "tteam_approver_revoke" => {
            let b: Holders = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"people\": [...], \"groups\": [...]}"))?;
            let grant = id == "tteam_approver_grant";
            let scope = c.params[1].clone();
            let ch = acc.change(|ro| ro.change_approvers(&who, ns, &scope, &b.people, &b.groups, grant)).map_err(change_err)?;
            ok(serde_json::json!({"ok": true}), line(ch))
        }
        "tteam_groups_get" => {
            let own = roles.groups.get(&owner(ns)).cloned().unwrap_or_default();
            let wide = if ns == "_tenant" { Default::default() } else { roles.groups.get(crate::cloudroles::TENANT_WIDE).cloned().unwrap_or_default() };
            ok(serde_json::json!({"team": ns, "groups": own, "tenant_wide": wide}), String::new())
        }
        "tteam_group_set" => {
            let b: GroupBody = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"members\": [people...]}"))?;
            let g = c.params[1].clone();
            let o = owner(ns);
            let ch = acc.change(|ro| ro.set_group(&who, &o, &g, &b.members)).map_err(change_err)?;
            ok(serde_json::json!({"ok": true}), line(ch))
        }
        _ => {
            let g = c.params[1].clone();
            let o = owner(ns);
            let ch = acc.change(|ro| ro.delete_group(&who, &o, &g)).map_err(change_err)?;
            ok(serde_json::json!({"ok": true}), line(ch))
        }
    }
}

/// Run start naming a provider that is a team's (or the tenant's) own `models.provider`, `integration.apikey` or `models.local` entry:
/// the model must be on its list and the person's counters must have room (`tenantconfig::runtime::provider_gate`). A provider no entry
/// is named after is not ours to gate (`Ok`); outside the cloud profile nothing changes.
pub fn provider_entry_gate(c: &Ctx, provider: &str, model: &str, team: Option<&str>) -> Result<(), Fail> {
    if !c.sh.profile.is_cloud() {
        return Ok(());
    }
    let dir = c.data_dir();
    let cfg = match crate::tenantconfig::store::Store::new(&dir).load() {
        Ok(cfg) => cfg,
        Err(_) => return Ok(()),
    };
    const KINDS: [&str; 3] = ["models.provider", "integration.apikey", "models.local"];
    let entries = cfg.entries();
    let Some(en) = entries.iter().find(|e| KINDS.contains(&e.item.kind.as_str()) && e.item.name == provider && (e.item.namespace.is_none() || e.item.namespace.as_deref() == team)) else { return Ok(()) };
    let kind = en.item.kind.clone();
    let me = &c.who().user;
    let roles = Access::new(&dir).roles().ok().flatten();
    if let Some(t) = team {
        if !roles.as_ref().is_some_and(|r| r.ns_role(t, me).is_some() || r.tenant_role(me).is_some_and(|x| x >= crate::cloudroles::TenantRole::Admin)) {
            return Err(Fail::new(404, "not_found", "no such team"));
        }
    }
    let caller = Caller { user: me, roles: roles.as_ref() };
    let tenant_admin = roles.as_ref().and_then(|r| r.tenant_role(me)).is_some_and(|x| x >= crate::cloudroles::TenantRole::Admin);
    let rank = team.map_or(if tenant_admin { 3 } else { 0 }, |t| caller.rank(t));
    let sink = Sink(c.sh.deps.audit.as_ref());
    let env = Env { dir: &dir, vault: crate::tenantconfig::vault::for_user(&dir, &me), audit: &sink };
    match crate::tenantconfig::runtime::provider_gate(&env, &kind, en.item.namespace.as_deref(), provider, me, rank, model, crate::tenantconfig::now_secs()) {
        crate::tenantconfig::enforce::AgentUse::Allowed { .. } => Ok(()),
        crate::tenantconfig::enforce::AgentUse::Refused(why) => Err(Fail::new(403, "denied", &why)),
    }
}
